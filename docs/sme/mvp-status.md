# Rust MVP Readiness Audit

**Source:** `nebo/` (Go, chi-conversion branch) | **Target:** `nebo-rs/` (Rust, Axum 0.8) | **Status:** Draft

This document assesses production-readiness of each MVP-critical subsystem in the
Rust rewrite. It consolidates the Go MVP SME (`nebo/docs/sme/MVP.md`) and the Rust
migration gap analysis (`nebo-rs/docs/sme/MIGRATION-SME.md`) into a single verdict
per subsystem.

Verdict scale:
- **SHIP IT** -- Subsystem is functionally complete and wired end-to-end.
- **SHIP WITH CAVEATS** -- Subsystem works but has known gaps that do NOT block launch.
- **BLOCKED** -- Subsystem has critical missing pieces that prevent shipping.
- **NOT STARTED** -- No meaningful Rust code exists for this subsystem.

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [HTTP Server](#2-http-server)
3. [WebSocket Hub](#3-websocket-hub)
4. [Authentication](#4-authentication)
5. [Agent Runner](#5-agent-runner)
6. [Tools](#6-tools)
7. [Orchestrator](#7-orchestrator)
8. [Lanes](#8-lanes)
9. [Memory and Embeddings](#9-memory-and-embeddings)
10. [MCP Bridge](#10-mcp-bridge)
11. [Browser](#11-browser)
12. [Voice](#12-voice)
13. [Updater](#13-updater)
14. [Apps](#14-apps)
15. [Skills](#15-skills)
16. [Janus Gateway](#16-janus-gateway)
17. [NeboLoop Integration](#17-neboloop-integration)
18. [Desktop](#18-desktop)
19. [Ship/Block Verdict Matrix](#19-shipblock-verdict-matrix)

---

## 1. Executive Summary

| # | Subsystem | Verdict | Blocking Issues |
|---|-----------|---------|-----------------|
| 2 | HTTP Server | SHIP WITH CAVEATS | ~47% route parity; missing store/plugin/dev/relay/MCP routes |
| 3 | WebSocket Hub | SHIP WITH CAVEATS | Single merged hub; no frame-type routing (event/res/stream/req) |
| 4 | Authentication | SHIP WITH CAVEATS | Login/register/refresh work; verify-email and password-reset routes now exist but email sender is NOT wired |
| 5 | Agent Runner | SHIP WITH CAVEATS | 14-step agentic loop present; missing skill auto-match, advisor deliberation, CLI providers |
| 6 | Tools | BLOCKED | 15 tools registered; Go has 70+ platform tools (0 in Rust); STRAP consolidation partial |
| 7 | Orchestrator | SHIP WITH CAVEATS | DAG-based sub-agent spawning works; crash recovery calls `recover()`; pending task persistence present |
| 8 | Lanes | SHIP IT | All 8 lane types with correct concurrency limits; pump tasks running |
| 9 | Memory and Embeddings | SHIP WITH CAVEATS | Hybrid search (FTS5 + vector) present; embedding providers exist; missing personality synthesis, file-based memory |
| 10 | MCP Bridge | SHIP WITH CAVEATS | Bridge + client + crypto work; NO MCP server, NO MCP OAuth |
| 11 | Browser | SHIP WITH CAVEATS | Chrome/CDP manager, actions, snapshots work; NO relay, NO extension bridge |
| 12 | Voice | NOT STARTED | Single comment stub |
| 13 | Updater | SHIP IT | Full pipeline: check, download, SHA256, health check, apply (Unix exec + Windows rename) |
| 14 | Apps | BLOCKED | 9 files (manifest, napp, registry, runtime, sandbox, signing, supervisor, hooks); missing gRPC adapters, install-from-URL, capability registration, store routes |
| 15 | Skills | SHIP WITH CAVEATS | Skill CRUD routes wired; missing store install/uninstall routes, fsnotify hot-reload, auto-match in runner |
| 16 | Janus Gateway | SHIP IT | Provider loaded from auth_profiles with janus metadata; X-Bot-ID header; streams via OpenAI compat |
| 17 | NeboLoop Integration | SHIP WITH CAVEATS | OAuth start/callback/status present; comm crate has loopback only, NO NeboLoop WS client |
| 18 | Desktop | NOT STARTED | Tauri is a separate project; no server-side desktop integration |

**Overall: Rust rewrite is NOT ready to ship as a full replacement.** The lane system,
updater, and Janus gateway are complete. The agent core (runner, orchestrator, memory,
steering) is substantially implemented. Critical blockers are platform tools (0 of 70+),
voice system (empty), and app platform gaps (no gRPC adapters, no store routes). A
"headless-only" MVP without voice, desktop automation, or app platform could ship with
caveats.

---

## 2. HTTP Server

### 2.1 Description

The HTTP server hosts all REST API endpoints, serves the SPA frontend via `rust-embed`,
and provides WebSocket upgrade endpoints for real-time communication. Built on Axum 0.8
with Tower middleware layers.

### 2.2 Go Implementation (Reference)

- **Framework:** chi router
- **Routes:** 200+ across 20 handler categories
- **Middleware:** JWT, CORS, CSRF, rate limiting, compression, security headers, ETag caching
- **SPA:** Embedded via `embed.FS`
- **Key file:** `internal/server/server.go`

### 2.3 Rust Implementation Status

| Feature | Status | Notes |
|---------|--------|-------|
| Axum 0.8 server | Y | `crates/server/src/lib.rs` -- `run()` function |
| Port binding + TcpListener check | Y | Checks availability before bind |
| CORS (localhost whitelist) | Y | Ports 27895, 5173, 4173 |
| Compression (tower-http) | Y | `CompressionLayer` on HTTP routes, excluded from WS |
| Security headers middleware | Y | `middleware::security_headers` |
| Rate limiter (auth routes) | Y | 10 req/min per IP on auth endpoints |
| JWT auth middleware | Y | On protected routes |
| Tracing layer | Y | `TraceLayer::new_for_http()` |
| SPA serving (rust-embed) | Y | `spa.rs` fallback handler |
| Health endpoint | Y | `GET /health` |
| Auth routes (login, register, refresh) | Y | Rate-limited |
| Auth routes (verify, forgot, reset) | Y | Routes wired; email sender NOT implemented |
| Setup routes | Y | 5 routes |
| Chat routes | Y | 11 routes including search, companion, days |
| Agent routes | Y | Sessions, settings, profile, status, lanes, advisors, channels |
| Memory routes | Y | CRUD + search + stats |
| Provider/model routes | Y | Full CRUD + test + task routing + CLI |
| Skill/extension routes | Y | CRUD + toggle + content |
| Task/cron routes | Y | CRUD + toggle + run + history |
| Integration (MCP) routes | Y | CRUD + test + registry + tools |
| Update routes | Y | Check + apply |
| File routes | Y | Browse + serve |
| NeboLoop OAuth routes | Y | Start, callback, status, account, janus usage |
| User routes (protected) | Y | CRUD + change-password |
| Notification routes (protected) | Y | List, read, read-all, delete, unread-count |
| Store/marketplace routes | N | Apps/skills store install/uninstall missing |
| Plugin routes | N | Plugin registry CRUD missing |
| Developer routes | N | Sideload, dev tools, browse-directory missing |
| Relay routes | N | Extension relay, CDP proxy missing |
| MCP server routes | N | `/mcp/*`, `/agent/mcp/*` missing |
| OAuth broker routes | N | Third-party OAuth URL/callback/disconnect missing |
| App UI routes | N | App UI proxy, static serve, open missing |
| CSRF middleware | N | -- |
| ETag caching | N | -- |

**Route count:** ~95 Rust vs ~200+ Go (~47% parity).

### 2.4 Verdict: SHIP WITH CAVEATS

Core API surface (chat, agent, memory, providers, skills, tasks, integrations, updates,
NeboLoop, user, notifications) is fully wired. Missing routes are for subsystems that
are themselves incomplete (store, plugins, dev, relay, MCP server, OAuth broker, app UI).

### 2.5 Key Blocking Issues

- Store/marketplace routes needed before apps or skills can be installed from NeboLoop
- Plugin registry CRUD needed for app/skill management persistence
- No CSRF protection (acceptable for localhost-only deployment)

---

## 3. WebSocket Hub

### 3.1 Description

Manages real-time bidirectional communication between the server and connected clients
(web UI, agent). Broadcasts events (chat_stream, update_available, lane status) and
handles approval/ask request-response flows.

### 3.2 Go Implementation (Reference)

- **Dual hub architecture:**
  - `internal/agenthub/hub.go` -- Agent hub (one WS connection, frame routing by type)
  - `internal/realtime/hub.go` -- Client hub (multiple browser WS connections, broadcast)
- **Frame types:** event, res, stream, req (with distinct routing per type)
- **Chat context handler:** Bridges agent events to client broadcasts
- **Agent-initiated requests:** Approval prompts, ask prompts with correlation IDs
- **Key files:** `internal/agenthub/hub.go`, `internal/realtime/hub.go`, `internal/realtime/chat.go`

### 3.3 Rust Implementation Status

| Feature | Status | Notes |
|---------|--------|-------|
| Client WebSocket handler | Y | `handlers/ws.rs` -- `client_ws_handler` |
| Agent WebSocket handler | Y | `handlers/ws.rs` -- `agent_ws_handler` |
| ClientHub broadcast | Y | Single merged hub for all clients |
| Message types (chat, cancel, ping) | Y | Parsed in WS handler |
| Session reset | Y | `session_reset` message type |
| Check stream | Y | `check_stream` message type |
| Approval response channels | Y | `approval_channels` in AppState |
| Ask response channels | Y | `ask_channels` in AppState |
| Frame-type routing (event/res/stream/req) | N | Single handler, no type-based dispatch |
| Separate agent hub | N | Merged into single ClientHub |
| Chat context handler | N | No bridge between agent events and client broadcasts |
| Rewrite handler | N | -- |
| Connection deduplication (reconnect drops old) | P | Basic handling, not fully enforced |

### 3.4 Verdict: SHIP WITH CAVEATS

The merged hub works for the current single-user local deployment model. Chat streaming,
approval flows, and event broadcasting are functional. The missing frame-type routing
becomes critical only when the full event pipeline (DM routing, channel bridging) is
needed.

### 3.5 Key Blocking Issues

- No frame-type dispatch means agent-initiated events and responses use the same path
- Chat context handler gap means DM events from NeboLoop cannot bridge to web UI
- These become blockers when NeboLoop comm plugin is implemented

---

## 4. Authentication

### 4.1 Description

JWT-based authentication with access/refresh token flow. Supports local account creation,
email verification, password reset, and NeboLoop OAuth integration.

### 4.2 Go Implementation (Reference)

- **Flows:** Register, login, refresh, verify-email, resend-verification, forgot-password, reset-password
- **JWT claims:** OwnerID, IsAdmin, Email, Plan
- **Email service:** Sends verification and reset emails
- **NeboLoop OAuth:** Consumer flow for linking Nebo accounts to NeboLoop
- **Key files:** `internal/logic/auth/`, `internal/handler/auth/`, `internal/middleware/jwt.go`

### 4.3 Rust Implementation Status

| Feature | Status | Notes |
|---------|--------|-------|
| JWT generation (access + refresh) | Y | `crates/auth/src/jwt.rs` |
| Login handler | Y | `handlers/auth.rs` |
| Register handler | Y | `handlers/auth.rs` |
| Refresh handler | Y | `handlers/auth.rs` |
| Forgot-password route | Y | Route wired at `/auth/forgot` |
| Reset-password route | Y | Route wired at `/auth/reset` |
| Verify-email route | Y | Route wired at `/auth/verify` |
| Resend-verification route | Y | Route wired at `/auth/resend` |
| Auth config endpoint | Y | Returns registration_enabled, require_email_verification |
| JWT middleware | Y | `middleware.rs` -- extracts and validates JWT |
| Rate limiting on auth routes | Y | 10 req/min per IP |
| Email sending service | N | No SMTP/email integration |
| NeboLoop OAuth consumer | P | OAuth start/callback wired; token management partial |
| Password hashing (argon2/bcrypt) | Y | In `auth/service.rs` |

### 4.4 Verdict: SHIP WITH CAVEATS

Core auth flow (register, login, refresh) is production-ready. Email verification and
password reset routes exist but are non-functional without an email sender -- acceptable
for local single-user deployment where the owner has direct DB access. NeboLoop OAuth
start and callback are wired, enabling Janus and comm features.

### 4.5 Key Blocking Issues

- Email sender NOT implemented -- verify-email and reset-password are dead routes
- NeboLoop OAuth token refresh not confirmed
- For single-user local app, email verification can be bypassed (mark verified in DB)

---

## 5. Agent Runner

### 5.1 Description

The agentic loop: receives a user prompt, builds context (system prompt + memories +
steering), calls the LLM, processes tool calls, and iterates until completion or limit.
Supports provider fallback, context compaction, and streaming.

### 5.2 Go Implementation (Reference)

- **14-step loop** in `internal/agent/runner/runner.go`
- **Provider fallback:** Exponential backoff, task-based model routing via `selector.go`
- **Context compaction:** Mid-conversation pruning when approaching token limits
- **Steering pipeline:** 10 generators inject ephemeral guidance
- **Skill auto-match:** Substring matching against triggers before each LLM call
- **Advisor deliberation:** Internal "voices" that weigh in before agent decides
- **Key files:** `internal/agent/runner/runner.go`, `internal/agent/steering/`, `internal/agent/advisors/`

### 5.3 Rust Implementation Status

| Feature | Status | Notes |
|---------|--------|-------|
| Agentic loop (max 100 iterations) | Y | `crates/agent/src/runner.rs` |
| Streaming via SSE events | Y | `ai/src/sse.rs` + runner stream callback |
| Provider fallback | Y | Multiple providers in `Arc<RwLock<Vec<Provider>>>` |
| Model selector (task routing) | Y | `crates/agent/src/selector.rs` with `ModelRoutingConfig` |
| Context pruning | Y | `crates/agent/src/pruning.rs` with `ContextThresholds` |
| System prompt assembly | Y | `crates/agent/src/prompt.rs` |
| DB context builder | Y | `crates/agent/src/db_context.rs` |
| Session management | Y | `crates/agent/src/session.rs` with `SessionManager` |
| Steering pipeline | P | 7+ generators: IdentityGuard, ChannelAdapter, ToolNudge, DateTimeRefresh, MemoryNudge, TaskParameterNudge, ObjectiveTaskNudge |
| Missing steering generators | N | compactionRecovery, taskProgress, janusQuotaWarning |
| Message deduplication | Y | `crates/agent/src/dedupe.rs` with `DedupeCache` |
| Fuzzy model matching | Y | `crates/agent/src/fuzzy.rs` |
| Key parser (session routing) | Y | `crates/agent/src/keyparser.rs` |
| Tool filter | Y | `crates/agent/src/tool_filter.rs` |
| Transcript builder | Y | `crates/agent/src/transcript.rs` |
| Context compaction | Y | `crates/agent/src/compaction.rs` |
| Sanitizer | Y | `crates/agent/src/sanitize.rs` |
| Skill auto-match in loop | N | Skills exist but NOT integrated into runner loop |
| Advisor deliberation | N | No advisor system in Rust |
| CLI providers (claude/gemini/codex) | N | CLIs detected but not used as providers |
| Origin propagation | Y | `RunRequest.origin` field |
| Transient retry (max 10) | Y | With 300s tool timeout |
| Tool execution timeout | Y | 300 seconds |

### 5.4 Verdict: SHIP WITH CAVEATS

The core agentic loop is solid -- iterates, streams, handles tool calls, prunes context,
selects models, and falls back across providers. The steering pipeline covers the most
important generators. Missing skill auto-match means skills must be manually loaded.
Missing advisors are a non-critical feature for MVP.

### 5.5 Key Blocking Issues

- Skill auto-match not wired into runner loop (skills work via manual `skill(action: load)`)
- No advisor deliberation system (deferred -- not in Go MVP audit either)
- 3 steering generators missing (compactionRecovery, taskProgress, janusQuotaWarning)

---

## 6. Tools

### 6.1 Description

The tool registry provides the agent with capabilities: file operations, shell execution,
web browsing, memory management, scheduling, inter-agent communication, and platform-
specific integrations (clipboard, contacts, calendar, etc.). Tools follow the STRAP
(Single Tool Resource Action Pattern) for reduced LLM context overhead.

### 6.2 Go Implementation (Reference)

- **118 tool files** across 30+ platform-specific tools
- **4 STRAP domain tools:** file, shell, web, agent (consolidating 35+ individual tools)
- **Platform tools:** Clipboard, contacts, calendar, mail, messages, music, reminders,
  keychain, accessibility, spotlight, spaces, dock, menubar, shortcuts, dialogs,
  window management, notifications, app management, desktop operations, system domain
- **Policy system:** Deny/allowlist/full with ask modes
- **Origin-based restrictions:** OriginUser, OriginComm, OriginApp, OriginSkill, OriginSystem
- **Key files:** `internal/agent/tools/` (118 files)

### 6.3 Rust Implementation Status

| Feature | Status | Notes |
|---------|--------|-------|
| Tool registry | Y | `crates/tools/src/registry.rs` |
| Domain tool trait | Y | `crates/tools/src/domain.rs` |
| Policy system (deny/allowlist/full) | Y | `crates/tools/src/policy.rs` |
| Origin system | Y | `crates/tools/src/origin.rs` with all 5 origin types |
| Safeguards | Y | `crates/tools/src/safeguard.rs` |
| Process registry | Y | `crates/tools/src/process.rs` |
| FileTool | Y | `file_tool.rs` -- read, write, edit, glob, grep |
| GrepTool (separate) | Y | `grep_tool.rs` |
| ShellTool | Y | `shell_tool.rs` -- exec, bg, kill, list |
| WebTool | Y | `web_tool.rs` -- requires browser manager |
| BotTool | Y | `bot_tool.rs` |
| EventTool | Y | `event_tool.rs` |
| MessageTool | Y | `message_tool.rs` |
| SkillTool | Y | `skill_tool.rs` |
| SystemTool | Y | `system_tool.rs` |
| DesktopTool | Y | `desktop_tool.rs` |
| SettingsTool | Y | `settings_tool.rs` |
| SpotlightTool | Y | `spotlight_tool.rs` |
| Orchestrator tool | Y | `orchestrator.rs` -- spawn sub-agents |
| Platform tools (macOS) | N | 0 of 20+ platform-specific tools |
| Platform tools (Linux) | N | 0 platform tools |
| Platform tools (Windows) | N | 0 platform tools |
| Memory tool (store/recall/search) | N | Not in tools crate (memory is in agent crate) |
| Cron/scheduler tool | N | Cron routes exist but no agent tool |
| Screenshot tool | N | -- |
| Vision tool | N | -- |
| TTS tool | N | -- |
| Desktop queue tool | N | -- |
| NeboLoop/Loop tools | N | -- |
| Query sessions tool | N | -- |

**Tool count:** ~15 Rust vs ~118 Go files (~13% parity).

### 6.4 Verdict: BLOCKED

The tool registry, domain trait, policy system, and origin system are well-implemented.
Core STRAP tools (file, shell, web) work. However, platform tools are the primary value
proposition of a desktop AI agent -- clipboard, contacts, calendar, mail, reminders,
accessibility, screenshots -- and ZERO are implemented in Rust. This is the single
largest gap in the migration.

### 6.5 Key Blocking Issues

- **CRITICAL:** 0 platform tools means the agent cannot interact with the user's OS
- No screenshot/vision means no computer-use capability
- No memory tool means agent cannot store/recall memories via tool calls (memory extraction
  from conversation works, but explicit `agent(resource: memory, action: store)` does not)
- No cron tool means agent cannot create scheduled tasks via conversation

---

## 7. Orchestrator

### 7.1 Description

Manages sub-agent spawning for parallel work. The main agent can decompose a task into
subtasks, spawn sub-agents (each with its own session), and collect results. Uses a DAG
for dependency ordering and a semaphore for concurrency control.

### 7.2 Go Implementation (Reference)

- **Max 5 concurrent sub-agents** (hard cap 10)
- **Pending task persistence:** `pending_tasks` table for crash recovery
- **Sub-agent sessions:** `subagent-{uuid}` format
- **Key files:** `internal/agent/orchestrator/orchestrator.go`, `internal/agent/recovery/`

### 7.3 Rust Implementation Status

| Feature | Status | Notes |
|---------|--------|-------|
| Orchestrator struct | Y | `crates/agent/src/orchestrator.rs` |
| Semaphore-based concurrency (max 10) | Y | `Arc<Semaphore>` |
| Active agent tracking | Y | `Arc<RwLock<HashMap<String, ActiveAgent>>>` |
| Lane integration | Y | `with_lanes()` builder method |
| Task decomposition | Y | `crates/agent/src/decompose.rs` |
| Task graph (DAG) | Y | `crates/agent/src/task_graph.rs` with `AgentType` |
| Orchestrator handle (late binding) | Y | `tools::new_handle()` in server startup |
| Recovery on startup | Y | `orch_handle.get().unwrap().recover().await` called in `run()` |
| SpawnRequest / SpawnResult types | Y | Exported from `crates/tools/src/orchestrator.rs` |
| SubAgentOrchestrator trait | Y | Trait object stored in handle |
| Pending task DB persistence | P | DB queries exist (`pending_tasks`); full persistence flow unclear |
| Crash log capture | N | -- |
| Result delivery back to parent | P | Via spawn result; streaming unclear |

### 7.4 Verdict: SHIP WITH CAVEATS

The orchestrator has the right architecture: DAG-based decomposition, semaphore-limited
concurrency, lane integration, and late-binding handle for the tool registry. Recovery
is called on startup. The main uncertainty is whether the full persistence and result
delivery pipeline has been tested end-to-end.

### 7.5 Key Blocking Issues

- End-to-end sub-agent flow needs integration testing
- Crash recovery completeness is uncertain (DB queries exist, recovery is called)
- No crash log capture

---

## 8. Lanes

### 8.1 Description

Multi-lane work queue system with per-lane concurrency limits, backpressure, and
lifecycle events. Each lane has its own pump task. Lanes organize different types of
work to prevent interference (e.g., serialized main lane prevents conversation corruption
from concurrent messages).

### 8.2 Go Implementation (Reference)

- **8 lane types:** main (2), events (unlimited), subagent (5), nested (3), heartbeat (1), comm (5), dev (1), desktop (1)
- **Hard caps:** nested=3, subagent=10
- **Pump goroutine per lane** with FIFO queue
- **Key file:** `internal/agenthub/lane.go`

### 8.3 Rust Implementation Status

| Feature | Status | Notes |
|---------|--------|-------|
| LaneManager struct | Y | `crates/agent/src/lanes.rs` |
| All 8 lane types | Y | main(2), events(0), subagent(5), nested(3), heartbeat(1), comm(5), dev(1), desktop(1) |
| Per-lane FIFO queue | Y | `VecDeque<LaneTask>` |
| Per-lane concurrency limits | Y | `max_concurrent` field, 0 = unlimited |
| Pump tasks (tokio::spawn per lane) | Y | `start_pumps()` method |
| LaneTask with completion channel | Y | `oneshot::Sender<Result<(), String>>` |
| Cancellation token | Y | `CancellationToken` for shutdown |
| Notify-based wake | Y | `Arc<Notify>` per lane |
| Warn-after timeout | Y | `warn_after_ms` field on `LaneTask` |
| Lane constants in types crate | Y | `types::constants::lanes::MAIN`, etc. |
| Hard caps | N | No `SetConcurrency()` with hard-cap enforcement |
| Enqueue options (functional pattern) | N | Direct enqueue only |
| Lane stats reporting | P | Structure exists; API endpoint at `/agent/lanes` |

### 8.4 Verdict: SHIP IT

All 8 lane types are implemented with correct default concurrency limits. Pump tasks
run per-lane, dispatching from FIFO queues with semaphore-like active tracking. The lane
manager is wired into the server at startup (`lanes.start_pumps()`) and passed to the
orchestrator. Missing hard caps and functional options are edge cases that do NOT affect
normal operation.

### 8.5 Key Blocking Issues

None. This subsystem is complete for MVP.

---

## 9. Memory and Embeddings

### 9.1 Description

The memory system extracts facts from conversations, stores them with vector embeddings,
and retrieves relevant context via hybrid search (FTS5 full-text + vector similarity).
Supports 3-tier memory (tacit, daily, entity), text chunking, and personality synthesis.

### 9.2 Go Implementation (Reference)

- **Extraction:** LLM-based fact extraction from conversation (preferences, entities, decisions, styles, artifacts, task_context)
- **Storage:** 3-tier (tacit/daily/entity) in embeddings table with vector blobs
- **Search:** Hybrid FTS5 + cosine similarity with adaptive weights
- **Chunking:** Text chunking for long documents
- **Personality:** Synthesis from accumulated style memories
- **Key files:** `internal/agent/embeddings/`, `internal/agent/memory/`

### 9.3 Rust Implementation Status

| Feature | Status | Notes |
|---------|--------|-------|
| Fact extraction types | Y | `crates/agent/src/memory.rs` -- `ExtractedFacts`, 6 categories |
| MemoryEntry storage type | Y | With layer, namespace, key, value, tags, confidence |
| Scored memory (decay + confidence) | Y | Ranking logic present |
| Hybrid search | Y | `crates/agent/src/search.rs` -- FTS5 + vector |
| Adaptive search weights | Y | `classify_query()` + `adaptive_weights()` |
| FTS5 on memories table | Y | `store.search_memories_fts()` |
| Vector similarity search | Y | Via embedding provider |
| Embedding provider trait | Y | `crates/ai/src/embedding.rs` -- `EmbeddingProvider` trait |
| OpenAI embedding provider | Y | `OpenAIEmbeddingProvider` (text-embedding-3-small) |
| Ollama embedding provider | Y | `OllamaEmbeddingProvider` |
| Cached embedding provider | Y | `CachedEmbeddingProvider` wrapper |
| Byte conversion (f32 <-> blob) | Y | `bytes_to_f32`, `f32_to_bytes` |
| Text chunking | Y | `crates/agent/src/chunking.rs` |
| Memory debounce | Y | `crates/agent/src/memory_debounce.rs` |
| Memory flush | Y | `crates/agent/src/memory_flush.rs` |
| Personality synthesis | Y | `crates/agent/src/personality.rs` |
| DB queries (memories, embeddings) | Y | `queries/memories.rs`, `queries/embeddings.rs` |
| Memory HTTP routes | Y | CRUD + search + stats (6 endpoints) |
| File-based memory | N | Go's file memory store not ported |
| Memory tool (agent tool) | N | No `agent(resource: memory)` tool |

### 9.4 Verdict: SHIP WITH CAVEATS

The memory subsystem is substantially implemented. Hybrid search with FTS5 and vector
similarity is functional. Embedding providers (OpenAI, Ollama) with caching are ready.
Fact extraction types, chunking, debounce, flush, and personality are all present. The
main gap is the agent tool interface -- memories are extracted from conversation but
the agent cannot explicitly store/recall/forget via tool calls.

### 9.5 Key Blocking Issues

- No `agent(resource: memory, action: store/recall/search/forget)` tool
- File-based memory not ported (lower priority)

---

## 10. MCP Bridge

### 10.1 Description

Model Context Protocol integration enabling Nebo to connect to external MCP servers
(database tools, API tools, etc.) and proxy their tools into the agent's tool registry.
Also includes an MCP server mode where Nebo exposes its own tools to external clients.

### 10.2 Go Implementation (Reference)

- **MCP client:** Connects to external MCP servers via stdio/SSE transports
- **MCP bridge:** Proxies external tools as `mcp__{server}__{tool}` in tool registry
- **MCP server:** JSON-RPC server exposing Nebo tools to external clients
- **MCP OAuth:** Authentication for MCP connections
- **Credential encryption:** AES-GCM for stored server credentials
- **Key files:** `internal/mcp/bridge/`, `internal/mcp/client/`, `internal/mcp/`

### 10.3 Rust Implementation Status

| Feature | Status | Notes |
|---------|--------|-------|
| MCP client | Y | `crates/mcp/src/client.rs` |
| MCP bridge | Y | `crates/mcp/src/bridge.rs` with `IntegrationInfo` |
| Proxy tool registration | Y | Tools registered as `mcp__{server}__{tool}` |
| Credential encryption (AES-GCM) | Y | `crates/mcp/src/crypto.rs` with `resolve_encryption_key` |
| MCP types | Y | `crates/mcp/src/types.rs` |
| Bridge sync on startup | Y | `bridge.sync_all()` from DB integrations |
| Integration HTTP routes | Y | CRUD + test + registry + tools (8 endpoints) |
| DB queries (mcp_integrations) | Y | `queries/mcp_integrations.rs` |
| Set bridge on tool registry | Y | `tool_registry.set_bridge(bridge)` |
| MCP server (JSON-RPC) | N | Nebo cannot expose tools to external clients |
| MCP OAuth handler | N | No OAuth for MCP connections |
| MCP context | N | No scoped context for MCP sessions |
| Agent MCP routes | N | `/agent/mcp/*` missing |

### 10.4 Verdict: SHIP WITH CAVEATS

The MCP client and bridge are functional -- users can connect external MCP servers and
their tools appear in the agent's registry. Credential encryption is implemented.
Integration management routes are complete. The missing MCP server means Nebo cannot
be used as an MCP provider by external tools, which is a secondary use case.

### 10.5 Key Blocking Issues

- No MCP server mode (Nebo cannot expose tools to external clients)
- No MCP OAuth (external servers requiring OAuth cannot authenticate)
- These are NOT blocking for MVP -- most MCP servers use API keys, not OAuth

---

## 11. Browser

### 11.1 Description

Headless Chrome automation via CDP (Chrome DevTools Protocol). The agent can navigate
web pages, interact with elements, capture accessibility snapshots, and extract content.
The relay mode allows a Chrome extension to bridge the user's actual browser session.

### 11.2 Go Implementation (Reference)

- **Chrome management:** Launch, connect, pool sessions
- **Actions:** Navigate, click, type, screenshot, scroll, wait
- **Snapshots:** Accessibility tree extraction + DOM analysis
- **Relay:** Chrome extension bridge for real browser sessions
- **CDP proxy:** Proxy CDP commands to relay-connected browsers
- **Storage:** Cookie/localStorage management
- **Key files:** `internal/browser/` (7 files), `internal/browser/relay.go`

### 11.3 Rust Implementation Status

| Feature | Status | Notes |
|---------|--------|-------|
| Browser manager | Y | `crates/browser/src/manager.rs` |
| Chrome detection + launch | Y | `crates/browser/src/chrome.rs` |
| Browser config | Y | `crates/browser/src/config.rs` with profiles |
| Session management | Y | `crates/browser/src/session.rs` |
| CDP actions | Y | `crates/browser/src/actions.rs` |
| Page snapshots | Y | `crates/browser/src/snapshot.rs` |
| Snapshot store | Y | `crates/browser/src/snapshot_store.rs` (in-memory) |
| Cookie/storage management | Y | `crates/browser/src/storage.rs` |
| WebTool integration | Y | `register_all_with_browser()` passes manager to tools |
| Element references | Y | `ElementRef` with id, role, name, selector |
| Console/error capture | Y | `ConsoleMessage`, `PageError` types |
| Extension relay | N | No Chrome extension bridge |
| CDP proxy | N | No relay CDP proxy |
| Relay WebSocket endpoint | N | `/relay/extension` etc. missing |
| Browser audit | N | -- |

### 11.4 Verdict: SHIP WITH CAVEATS

Headless browser automation works: the agent can navigate pages, interact with elements,
and capture accessibility snapshots via CDP. The snapshot store provides in-memory caching
of accessibility trees. Missing relay means users cannot let the agent interact with
their actual browser sessions -- the agent only works with its own headless Chrome
instances.

### 11.5 Key Blocking Issues

- No extension relay means agent cannot see or control the user's browser tabs
- Relay is a significant UX feature but NOT a launch blocker
- Headless automation covers the core use case (web research, form filling)

---

## 12. Voice

### 12.1 Description

Voice pipeline: speech-to-text (Whisper ASR), text-to-speech (Kokoro TTS), voice
activity detection (Silero+RMS VAD), wakeword detection, duplex audio, and phonemization.
Enables hands-free interaction with the agent.

### 12.2 Go Implementation (Reference)

- **22 files** in `internal/voice/`
- **ASR:** Whisper model (ONNX runtime)
- **TTS:** Kokoro model with phonemization
- **VAD:** Silero (ONNX) + RMS energy detection
- **Wakeword:** Custom detection
- **Duplex audio:** Simultaneous listen + speak
- **WS endpoints:** `/ws/voice`, `/ws/voice/wake`
- **HTTP endpoints:** `POST /api/v1/voice/transcribe`, `POST /api/v1/voice/tts`, `GET /api/v1/voice/voices`

### 12.3 Rust Implementation Status

| Feature | Status | Notes |
|---------|--------|-------|
| Voice crate | N | `crates/voice/src/lib.rs` contains a single comment: `// Voice pipeline - Whisper ASR, TTS, VAD (Phase 7)` |
| ASR (Whisper) | N | -- |
| TTS (Kokoro) | N | -- |
| VAD (Silero/RMS) | N | -- |
| Wakeword detection | N | -- |
| Duplex audio | N | -- |
| Voice WebSocket endpoints | N | -- |
| Voice HTTP endpoints | N | -- |
| ONNX runtime integration | N | -- |

### 12.4 Verdict: NOT STARTED

The voice crate is an empty stub with a single comment. This is 0% implemented. The Go
implementation is 22 files with ONNX model integrations, making this one of the most
complex subsystems to port.

### 12.5 Key Blocking Issues

- **CRITICAL:** Entire subsystem missing
- ONNX runtime bindings needed (C FFI or Rust native crate)
- Whisper, Kokoro, and Silero model downloads and management
- Audio capture and playback platform abstractions
- This is a Phase 7 item per the stub comment -- intentionally deferred

---

## 13. Updater

### 13.1 Description

Self-update system: checks a CDN for newer versions, downloads the binary, verifies
SHA256 checksums, runs a health check, and applies the update by replacing the current
binary. Supports Unix (execve in-place) and Windows (rename + spawn) apply strategies.

### 13.2 Go Implementation (Reference)

- **Flow:** Check -> Download -> SHA256 Verify -> Health Check -> Apply -> Rollback on failure
- **BackgroundChecker:** 6h interval, 30s initial delay
- **Install detection:** direct, homebrew, package_manager
- **WS events:** update_available, update_progress, update_ready, update_error
- **Key files:** `internal/updater/updater.go`, `internal/updater/apply_unix.go`, `apply_windows.go`

### 13.3 Rust Implementation Status

| Feature | Status | Notes |
|---------|--------|-------|
| Version check (CDN) | Y | `check()` with 5s timeout, `version.json` |
| Version comparison | Y | `normalize_version()`, `is_newer()` -- semver without pre-release |
| Install method detection | Y | direct/homebrew/package_manager |
| Download with progress | Y | Streaming download with `ProgressFn` callback |
| SHA256 verification | Y | `verify_checksum()` against `checksums.txt` |
| Health check | Y | `nebo --version` on new binary |
| Apply (Unix) | Y | Backup -> copy -> execve -> rollback on failure |
| Apply (Windows) | Y | Rename -> copy -> spawn new process -> exit |
| Pre-apply hooks | Y | `set_pre_apply_hook()` for resource cleanup |
| BackgroundChecker | Y | Configurable interval, 30s initial delay |
| Dedup notification | Y | `last_notified` tracks already-notified versions |
| HTTP endpoints | Y | `GET /update/check`, `POST /update/apply` |
| WS event broadcasting | Y | update_available, update_progress, update_ready, update_error |
| Auto-download for direct installs | Y | Triggered from BackgroundChecker callback |
| Version "dev" guard | Y | Never considered outdated |

### 13.4 Verdict: SHIP IT

The updater is fully implemented and matches Go parity. Check, download, verify, health
check, and apply are all wired. Both Unix (execve) and Windows (rename+spawn) apply
strategies are implemented. The background checker runs at 1h intervals (Go uses 6h --
minor difference) with 30s initial delay. WS events broadcast update progress to the
frontend.

### 13.5 Key Blocking Issues

None. This subsystem is complete.

---

## 14. Apps

### 14.1 Description

The app platform allows third-party sandboxed applications (.napp packages) to extend
Nebo with custom capabilities. Apps are downloaded from NeboLoop, verified with ED25519
signatures, extracted, and launched as separate processes communicating via gRPC over
Unix sockets. Apps can provide 9 capability types (gateway, tool, vision, comm, channel,
UI, schedule, hooks, browser).

### 14.2 Go Implementation (Reference)

- **44 files** in `internal/apps/`
- **Pipeline:** Download -> ED25519 verify -> extract -> launch -> register capabilities
- **gRPC adapters:** 8 adapter types bridging gRPC to Nebo interfaces
- **Supervisor:** Exponential backoff restart, quarantine on budget exhaustion
- **Install events:** Via NeboLoop WebSocket
- **Store routes:** Install/uninstall/list from marketplace
- **Key files:** `internal/apps/registry.go`, `internal/apps/runtime.go`, `internal/apps/adapter.go`

### 14.3 Rust Implementation Status

| Feature | Status | Notes |
|---------|--------|-------|
| App manifest types | Y | `crates/apps/src/manifest.rs` |
| Napp extraction | Y | `crates/apps/src/napp.rs` |
| App registry | P | `crates/apps/src/registry.rs` -- basic structure |
| App runtime | P | `crates/apps/src/runtime.rs` -- process lifecycle |
| Sandbox (env sanitization) | Y | `crates/apps/src/sandbox.rs` |
| ED25519 signing | Y | `crates/apps/src/signing.rs` with `SigningKeyProvider`, `RevocationChecker` |
| Supervisor | P | `crates/apps/src/supervisor.rs` -- basic watchdog |
| Hooks | Y | `crates/apps/src/hooks.rs` |
| Install event types | Y | `InstallEvent`, `QuarantineEvent` types |
| gRPC adapters | N | No protobuf/gRPC integration |
| Capability registration (9 types) | N | No adapter pattern for gateway/tool/vision/comm/channel/UI/schedule/hooks/browser |
| Install from URL | N | Download pipeline not wired |
| File watcher | N | No fsnotify equivalent for app directory |
| Per-app settings store | N | -- |
| Store HTTP routes | N | Install/uninstall/list from marketplace missing |
| App UI routes | N | Proxy, static serve, open window missing |
| Boot discovery (scan + launch) | N | Not confirmed |
| Permission diff on update | N | -- |

### 14.4 Verdict: BLOCKED

The app crate has the right type foundations (manifest, napp extraction, signing, sandbox)
but lacks the critical runtime glue: gRPC adapters for capability registration, install-
from-URL pipeline, store routes, and app UI routes. Without these, apps cannot be
installed, launched, or interact with the agent.

### 14.5 Key Blocking Issues

- **CRITICAL:** No gRPC adapters -- apps cannot register capabilities
- No store routes -- apps cannot be installed from NeboLoop marketplace
- No install-from-URL pipeline -- apps cannot be downloaded
- No app UI routes -- apps with UI capability cannot serve their interfaces

---

## 15. Skills

### 15.1 Description

Skills are YAML-defined prompt templates that augment the agent's capabilities. They can
be bundled (embedded at compile time), installed from the NeboLoop store, or user-created.
Skills are matched by triggers and injected into the system prompt for scoped sessions.

### 15.2 Go Implementation (Reference)

- **SkillDomainTool:** 612 lines handling load/unload/list, trigger matching, content injection
- **Auto-match:** Case-insensitive substring matching in runner loop
- **Hot-reload:** fsnotify watcher on skills directory
- **Store routes:** Install/uninstall from NeboLoop marketplace
- **Session-scoped:** TTL expiry (4 turns auto, 6 turns manual)
- **Key files:** `internal/agent/tools/skill_tool.go`, `internal/agent/skills/`

### 15.3 Rust Implementation Status

| Feature | Status | Notes |
|---------|--------|-------|
| Skill CRUD routes | Y | Create, get, update, delete, toggle, content |
| Extension list endpoint | Y | `GET /extensions` |
| SkillTool in tools crate | Y | `crates/tools/src/skill_tool.rs` |
| Skill tool (load/unload/list) | P | Tool exists; full SKILL.md injection unclear |
| Trigger matching | N | Auto-match not in runner loop |
| Hot-reload (fsnotify) | N | No file watcher on skills directory |
| Store install/uninstall | N | No marketplace routes |
| Bundled skills (embedded) | N | Not confirmed whether embedded at compile time |
| Session TTL expiry | N | No turn-based auto-unload |
| Budget (MaxActiveSkills=4, MaxSkillTokenBudget=16K) | N | Not confirmed |

### 15.4 Verdict: SHIP WITH CAVEATS

Skill management routes are wired (CRUD, toggle, content). The SkillTool exists in the
tools crate. However, the integration with the runner loop (auto-match, content injection,
TTL expiry) is missing or incomplete. Skills can be managed via the UI but may not
function correctly in conversation without manual loading.

### 15.5 Key Blocking Issues

- No trigger-based auto-match in runner loop
- No fsnotify hot-reload
- No store install/uninstall routes
- Skills are manageable but not fully functional in the agentic loop

---

## 16. Janus Gateway

### 16.1 Description

Janus is NeboLoop's AI gateway that routes LLM requests to upstream providers (Anthropic,
OpenAI, Gemini). Nebo connects to Janus using a JWT and bot ID, enabling pay-as-you-go
AI usage without users needing their own API keys.

### 16.2 Go Implementation (Reference)

- **Provider loading:** `janus_provider=true` metadata on NeboLoop auth profiles
- **Headers:** `Authorization: Bearer <JWT>`, `X-Bot-ID: <uuid>`, `X-Lane: <lane>`
- **Rate limiting:** 6 response headers -> `RateLimitInfo`, persisted to disk
- **4 streaming quirks:** Tool name duplication, complete JSON args, missing DONE sentinel, non-null content
- **Key files:** `cmd/nebo/providers.go`, `internal/agent/ai/api_openai.go`

### 16.3 Rust Implementation Status

| Feature | Status | Notes |
|---------|--------|-------|
| Janus provider detection | Y | Checks `janus_provider=true` in auth_profile metadata |
| OpenAI-compatible provider | Y | `OpenAIProvider::with_base_url()` pointing to Janus URL |
| Provider ID "janus" | Y | `p.set_provider_id("janus")` |
| X-Bot-ID header | Y | `p.set_bot_id(bot_id)` |
| Janus URL from config | Y | `cfg.neboloop.janus_url` (NOT from auth_profile) |
| Bot ID from data dir | Y | `config::read_bot_id()` |
| Streaming via OpenAI SSE | Y | Standard SSE streaming in OpenAI provider |
| Usage HTTP endpoint | Y | `GET /neboloop/janus/usage` |
| Rate limit capture | P | Headers may be captured in OpenAI provider; disk persistence unclear |
| X-Lane header | N | Lane name not sent in headers |
| Streaming quirk workarounds | P | Standard OpenAI streaming; Janus-specific quirks may not be handled |
| Quota warning steering | N | `janusQuotaWarning` generator missing |
| Disk persistence of usage | N | Not confirmed |

### 16.4 Verdict: SHIP IT

The Janus gateway is functional: provider detection from NeboLoop auth profiles, proper
URL routing through config, bot ID header, and standard OpenAI-compatible streaming. The
usage endpoint is wired. Missing X-Lane header and quota warning are non-critical for
MVP operation -- Janus still routes requests correctly without them.

### 16.5 Key Blocking Issues

- X-Lane header missing (analytics only, not functional)
- Quota warning steering generator missing (user may not see usage warnings)
- Rate limit disk persistence unconfirmed (usage resets on restart)

---

## 17. NeboLoop Integration

### 17.1 Description

NeboLoop is the cloud backend for Nebo. Integration includes OAuth for account linking,
the comm plugin for real-time messaging (DMs, loop channels), bot registration, app/skill
store access, and the Janus AI gateway.

### 17.2 Go Implementation (Reference)

- **OAuth consumer:** Login/link via NeboLoop, callback handling, token storage
- **Comm plugin:** WebSocket gateway client, bot auth, DM handling, channel messaging, auto-reconnect
- **Store SDK:** Install/uninstall apps and skills from marketplace
- **Bot registration:** Unique bot_id, card publishing
- **Key files:** `internal/neboloop/`, `internal/agent/comm/neboloop/plugin.go`

### 17.3 Rust Implementation Status

| Feature | Status | Notes |
|---------|--------|-------|
| OAuth start | Y | `GET /neboloop/oauth/start` |
| OAuth callback | Y | `GET /auth/neboloop/callback` (top-level route) |
| OAuth status | Y | `GET /neboloop/oauth/status` |
| Account status | Y | `GET /neboloop/account` |
| Account disconnect | Y | `DELETE /neboloop/account` |
| Bot status | Y | `GET /neboloop/status` |
| Open NeboLoop | Y | `GET /neboloop/open` |
| Janus usage | Y | `GET /neboloop/janus/usage` |
| Comm plugin interface | Y | `crates/comm/src/lib.rs` -- `CommPlugin` trait with lifecycle, messaging, registration |
| Loopback plugin | Y | `crates/comm/src/loopback.rs` for testing |
| Plugin manager | Y | `crates/comm/src/manager.rs` |
| Loop channel lister | Y | `LoopChannelLister` trait |
| Loop lister | Y | `LoopLister` trait |
| Channel message lister | Y | `ChannelMessageLister` trait |
| Channel member lister | Y | `ChannelMemberLister` trait |
| NeboLoop WS client | N | No WebSocket gateway client |
| Bot auth (WS CONNECT frame) | N | -- |
| DM send/receive | N | -- |
| Channel messaging | N | -- |
| Auto-reconnect | N | -- |
| Owner DM routing to main lane | N | -- |
| Store SDK (install apps/skills) | N | -- |
| App install event handling | N | -- |

### 17.4 Verdict: SHIP WITH CAVEATS

OAuth and account management routes are wired. The comm crate has a well-designed plugin
interface with traits for all communication patterns. However, the actual NeboLoop
WebSocket client -- the implementation that connects to `wss://comms.neboloop.com/ws` --
does NOT exist. Only the loopback plugin (for testing) is implemented. This means Nebo
cannot send or receive DMs, participate in loops, or receive store install events.

### 17.5 Key Blocking Issues

- **CRITICAL for comms:** No NeboLoop WS client means no DMs, no channels, no store events
- Store SDK missing means apps/skills cannot be installed from marketplace
- OAuth works, so Janus gateway IS functional (most critical NeboLoop integration)
- For headless-only MVP without comms, this is a caveat rather than a blocker

---

## 18. Desktop

### 18.1 Description

Native desktop integration: native window management, system tray, cursor simulation,
fingerprint tracking, JS injection into webviews, and window lock system. The Go version
uses Wails v3; the Rust plan is Tauri.

### 18.2 Go Implementation (Reference)

- **12 files** in `internal/webview/`
- **Wails v3:** Native window + system tray, close-to-tray behavior
- **Webview manager:** Window creation, resize, focus, JS injection
- **Cursor management:** Simulated cursor for computer-use
- **Fingerprint:** Browser fingerprint tracking
- **Desktop queue:** Serialized desktop automation (one screen, one mouse)
- **Key files:** `internal/webview/`, `cmd/nebo/desktop.go`

### 18.3 Rust Implementation Status

| Feature | Status | Notes |
|---------|--------|-------|
| Tauri integration | N | Tauri is a separate project, not integrated into nebo-rs |
| Native window management | N | -- |
| System tray | N | -- |
| Cursor simulation | N | -- |
| JS injection | N | -- |
| Window lock system | N | -- |
| Desktop queue (serialized) | N | Lane exists (desktop lane) but no desktop automation |
| Fingerprint tracking | N | -- |

### 18.4 Verdict: NOT STARTED

Desktop integration does not exist in the Rust codebase. The desktop lane is defined
(concurrency=1) for future use, but there is no Tauri/native window code in the server.
This is expected -- the Rust rewrite currently targets headless mode, and Tauri
integration is a separate project phase.

### 18.5 Key Blocking Issues

- **Entire subsystem missing** -- expected for headless-only MVP
- Tauri integration requires separate project setup (unlike Wails which embedded into Go)
- Desktop lane is reserved but unused

---

## 19. Ship/Block Verdict Matrix

### 19.1 Full Matrix

| # | Subsystem | Go Status | Rust Status | Verdict |
|---|-----------|-----------|-------------|---------|
| 2 | HTTP Server | Y | P (~47% routes) | SHIP WITH CAVEATS |
| 3 | WebSocket Hub | Y | P (merged hub) | SHIP WITH CAVEATS |
| 4 | Authentication | Y | P (no email sender) | SHIP WITH CAVEATS |
| 5 | Agent Runner | Y | P (no auto-match, no advisors) | SHIP WITH CAVEATS |
| 6 | Tools | Y | P (~13% tools) | BLOCKED |
| 7 | Orchestrator | Y | P (architecture done, testing needed) | SHIP WITH CAVEATS |
| 8 | Lanes | Y | Y (all 8 types) | SHIP IT |
| 9 | Memory and Embeddings | Y | P (no memory tool) | SHIP WITH CAVEATS |
| 10 | MCP Bridge | Y | P (no server, no OAuth) | SHIP WITH CAVEATS |
| 11 | Browser | Y | P (no relay) | SHIP WITH CAVEATS |
| 12 | Voice | Y | N (empty stub) | NOT STARTED |
| 13 | Updater | Y | Y (full parity) | SHIP IT |
| 14 | Apps | Y | P (types only) | BLOCKED |
| 15 | Skills | Y | P (CRUD only) | SHIP WITH CAVEATS |
| 16 | Janus Gateway | Y | Y (functional) | SHIP IT |
| 17 | NeboLoop Integration | Y | P (OAuth only) | SHIP WITH CAVEATS |
| 18 | Desktop | Y | N (separate project) | NOT STARTED |

### 19.2 Summary Counts

| Verdict | Count | Subsystems |
|---------|-------|------------|
| SHIP IT | 3 | Lanes, Updater, Janus Gateway |
| SHIP WITH CAVEATS | 11 | HTTP Server, WebSocket Hub, Auth, Agent Runner, Orchestrator, Memory, MCP Bridge, Browser, Skills, NeboLoop, Tools (partial) |
| BLOCKED | 2 | Tools (platform), Apps (runtime) |
| NOT STARTED | 2 | Voice, Desktop |

### 19.3 Critical Path to Ship

To achieve a "headless-only MVP" (no voice, no desktop, no app platform):

1. **Wire memory tool** -- Enable `agent(resource: memory, action: store/recall)` in
   tools crate. Estimated: 1-2 days.

2. **Wire skill auto-match** -- Integrate trigger matching into runner loop. Estimated:
   1 day.

3. **Add store routes** -- Marketplace install/uninstall for apps and skills. Estimated:
   2-3 days.

4. **Platform tools (Phase 1)** -- Start with clipboard, notifications, screenshot.
   These are the minimum platform tools needed for basic desktop agent utility.
   Estimated: 1-2 weeks.

5. **NeboLoop WS client** -- Implement the comm plugin for NeboLoop WebSocket gateway.
   Required for DMs, channels, and store events. Estimated: 1 week.

### 19.4 What Can Ship Today (Headless Chat Agent)

Even with current gaps, the Rust binary functions as a headless chat agent:

- User connects via web UI at `http://localhost:27895`
- WebSocket chat streaming works
- Agent runs agentic loop with tool calls (file, shell, web, grep)
- Memory extraction and hybrid search provide context
- Multiple AI providers (Anthropic, OpenAI, Ollama, DeepSeek, Google, Janus)
- Model selection with task routing and fallback
- Context compaction and pruning
- MCP integrations for external tool servers
- Scheduled tasks via cron
- Self-update via CDN
- NeboLoop account linking and Janus AI gateway

### 19.5 What Cannot Ship Yet

- **Platform integration:** Agent cannot access clipboard, contacts, calendar, mail,
  reminders, or any OS-level resources
- **Voice:** No speech input or output
- **Desktop automation:** No screenshot, window management, or cursor control
- **App platform:** Cannot install or run third-party apps
- **NeboLoop messaging:** Cannot send or receive DMs or participate in loops
- **Browser relay:** Cannot interact with user's actual browser tabs

---

## Appendix A: Crate Inventory

The Rust workspace contains 16 crates (the memory notes 18, but the actual count at
time of audit is 16):

| Crate | Purpose | Files | Status |
|-------|---------|-------|--------|
| `types` | Shared types, API types, constants, errors | 4 | Complete |
| `config` | Config loading, models.yaml, CLI detection, settings | 5 | Complete |
| `db` | SQLite via r2d2, migrations, 16 query modules, store | 20 | Substantial |
| `auth` | JWT generation/validation, auth service | 3 | Complete |
| `ai` | Provider trait, Anthropic/OpenAI/Ollama, embeddings, SSE | 7 | Complete |
| `tools` | Tool registry, domain tools, policy, origin, 15+ tools | 18 | Partial |
| `agent` | Runner, orchestrator, lanes, memory, steering, sessions | 24 | Substantial |
| `server` | Axum server, handlers, middleware, scheduler, SPA | 17 | Substantial |
| `mcp` | MCP client, bridge, crypto, types | 5 | Partial |
| `apps` | Manifest, napp, registry, runtime, sandbox, signing | 8 | Partial |
| `browser` | Chrome/CDP, actions, snapshots, storage | 8 | Substantial |
| `voice` | Empty stub | 1 | Not started |
| `comm` | Comm plugin trait, loopback, manager | 4 | Partial |
| `notify` | OS notifications (macOS/Linux/Windows) | 1 | Complete |
| `updater` | Check, download, verify, apply (Unix/Windows) | 2 | Complete |
| `cli` | CLI entrypoint | 1 | Minimal |

**Total: ~141 Rust source files vs ~498 Go internal files (~28% file parity).**

---

## Appendix B: Test Coverage

- **72 tests** passing across all crates (as of last known state)
- Tests cover: version comparison, install detection, sanitization, notification, and
  various unit-level behaviors
- No integration test suite for end-to-end agent flows
- No benchmark suite
- Test coverage is significantly below Go baseline

---

## Appendix C: Database Parity

| Metric | Go | Rust | Parity |
|--------|-----|------|--------|
| Migrations | 46 | 47 | 100%+ (Rust added 1) |
| Query modules | 26 | 16 | ~62% |
| Missing query modules | -- | -- | plugins, dev_apps, embeddings (some), voice_models, oauth |

Database schema is at full parity (migrations complete). Query modules lag because they
are written on-demand as handlers are implemented.

---

*Generated: 2026-03-04*
