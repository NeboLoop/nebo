# Nebo Migration SME: Go to Rust

**Source:** `/Users/almatuck/workspaces/nebo/nebo` (Go)
**Target:** `/Users/almatuck/workspaces/nebo/nebo-rs` (Rust)
**Purpose:** Windows flags Go binaries as viruses. Rust eliminates this.
**Rule:** No logic or behavior may be lost.

---

## Deep-Dive Logic Documents

Each file below contains the **exact algorithms, data structures, constants, state machines, and behavior** from the Go source -- sufficient to reimplement in Rust without referencing the Go code.

| # | Document | Covers | Size |
|---|---|---|---|
| 1 | [lanes-and-hub.md](lanes-and-hub.md) | Lane concurrency (8 types), Agent Hub, WebSocket lifecycle, frame routing | 39KB |
| 2 | [orchestrator-and-recovery.md](orchestrator-and-recovery.md) | Sub-agent spawning (max 5), crash recovery, pending tasks, result delivery | 38KB |
| 3 | [embeddings-and-memory.md](embeddings-and-memory.md) | Vector embeddings, hybrid search, text chunking, memory extraction, personality synthesis | 42KB |
| 4 | [voice-system.md](voice-system.md) | ASR (Whisper), TTS (Kokoro), VAD (Silero/RMS), wakeword, duplex audio, phonemization | 50KB |
| 5 | [platform-tools.md](platform-tools.md) | 70+ platform tools (macOS/Linux/Windows), STRAP pattern, snapshot pipeline, desktop queue | 75KB |
| 6 | [auth-and-app-platform.md](auth-and-app-platform.md) | Auth flows (JWT, email verify, password reset), .napp runtime, gRPC, sandbox, signing | 73KB |
| 7 | [browser-and-relay.md](browser-and-relay.md) | Chrome/CDP management, extension relay, actions, snapshots, storage, audit | 53KB |
| 8 | [mcp-and-comm.md](mcp-and-comm.md) | MCP server/client/bridge/OAuth, NeboLoop comm plugin (1541 LOC), A2A tasks | 71KB |
| 9 | [plugins-store-oauth-dev.md](plugins-store-oauth-dev.md) | Plugin system, NeboLoop store, OAuth broker, app OAuth, developer routes | 51KB |
| 10 | [infra-systems.md](infra-systems.md) | Message bus, events (pub/sub), keyring, credentials, notifications, heartbeat, real-time hub | 53KB |
| 11 | [desktop-and-webview.md](desktop-and-webview.md) | Wails integration, webview manager, cursor simulation, fingerprint, JS injection, lock system | 49KB |
| 12 | [agent-tools.md](agent-tools.md) | Tool registry, 20+ domain tools, safeguards, policy, process management, STRAP dispatch | 45KB |
| 13 | [agent-core.md](agent-core.md) | Runner (14-step loop), steering (12 generators), session, skills, advisors, 7 AI providers | 61KB |
| 14 | [middleware-and-config.md](middleware-and-config.md) | All middleware (JWT/CORS/CSRF/rate-limit), security, config, updater, service context | 73KB |
| 15 | [handlers-cli-db.md](handlers-cli-db.md) | 60+ HTTP handlers, 13 CLI commands, SQLite layer, 120+ SQL queries | 73KB |
| 16 | [misc-systems.md](misc-systems.md) | AFV (prompt injection defense), local models, agent MCP, bundled extensions, protobuf | 55KB |

## Unified System References

| # | Document | Covers | Size |
|---|---|---|---|
| 17 | [MEMORY_AND_PROMPT.md](MEMORY_AND_PROMPT.md) | Complete memory + prompt system: 31 sections covering storage, extraction, embeddings, hybrid search, prompt assembly, steering, compaction, with Rust status per section | ~90KB |

## Consolidated Migration References

These documents consolidate multiple Go SME docs into unified Rust migration references, covering systems where several Go docs map to a single Rust concern.

| # | Document | Consolidates (Go SME) | Size |
|---|---|---|---|
| 18 | [chat-and-streaming.md](chat-and-streaming.md) | AGENT_INPUT + CHAT_DISPLAY + CHAT_SYSTEMS + WEBFORMS | ~45KB |
| 19 | [task-system.md](task-system.md) | TASK_SYSTEM | ~25KB |
| 20 | [identity-and-onboarding.md](identity-and-onboarding.md) | BOT_IDENTITY + ONBOARDING + INTRODUCTION | ~30KB |
| 21 | [janus-and-providers.md](janus-and-providers.md) | JANUS_GATEWAY + LOCAL_INFERENCE | ~25KB |
| 22 | [file-and-integrations.md](file-and-integrations.md) | FILE_SERVING + INTEGRATIONS | ~25KB |
| 23 | [concurrency-patterns.md](concurrency-patterns.md) | CONCURRENCY | ~12KB |
| 24 | [deployment-and-build.md](deployment-and-build.md) | DEPLOYMENT | ~25KB |
| 25 | [mvp-status.md](mvp-status.md) | MVP (fresh Rust readiness audit) | ~30KB |

## New Feature Specs (not in Go -- must be built fresh)

| # | Document | Covers | Size |
|---|---|---|---|
| 26 | [platform-taxonomy.md](platform-taxonomy.md) | Canonical spec: Skills, Tools, Workflows, Roles, install codes, marketplace, execution model | 22KB |
| 27 | [workflow-engine.md](workflow-engine.md) | Implementation SME: Rust structs, lean execution algorithm, DB schema, API routes, crate design | 14KB |

**Total documentation: ~1.3MB across 27 files + this index.**

---

## Go SME Coverage Mapping

Every Go SME document now has corresponding Rust migration coverage. The 27 Go docs map to Rust docs as follows:

| # | Go SME Document | Rust Equivalent | Coverage |
|---|---|---|---|
| 1 | AGENT_INPUT.md | chat-and-streaming.md (#18) | Full |
| 2 | CHAT_DISPLAY.md | chat-and-streaming.md (#18) | Full |
| 3 | CHAT_SYSTEMS.md | chat-and-streaming.md (#18) | Full |
| 4 | WEBFORMS.md | chat-and-streaming.md (#18) | Full |
| 5 | TASK_SYSTEM.md | task-system.md (#19) | Full |
| 6 | BOT_IDENTITY.md | identity-and-onboarding.md (#20) | Full |
| 7 | ONBOARDING.md | identity-and-onboarding.md (#20) | Full |
| 8 | INTRODUCTION.md | identity-and-onboarding.md (#20) | Full |
| 9 | JANUS_GATEWAY.md | janus-and-providers.md (#21) | Full |
| 10 | LOCAL_INFERENCE.md | janus-and-providers.md (#21) | Full |
| 11 | FILE_SERVING.md | file-and-integrations.md (#22) | Full |
| 12 | INTEGRATIONS.md | file-and-integrations.md (#22) | Full |
| 13 | CONCURRENCY.md | concurrency-patterns.md (#23) | Full |
| 14 | DEPLOYMENT.md | deployment-and-build.md (#24) | Full |
| 15 | MVP.md | mvp-status.md (#25) | Full |
| 16 | COMMS.md | mcp-and-comm.md (#8) | Full |
| 17 | UPDATER.md | middleware-and-config.md (#14) | Full |
| 18 | APPS_RUNTIME.md | auth-and-app-platform.md (#6) | Full |
| 19 | VOICE.md | voice-system.md (#4) | Full |
| 20 | BROWSER.md | browser-and-relay.md (#7) | Full |
| 21 | MEMORY.md | embeddings-and-memory.md (#3) + MEMORY_AND_PROMPT.md (#17) | Full |
| 22 | STEERING.md | agent-core.md (#13) | Full |
| 23 | TOOLS.md | agent-tools.md (#12) + platform-tools.md (#5) | Full |
| 24 | SESSION.md | agent-core.md (#13) | Full |
| 25 | SKILLS.md | agent-core.md (#13) | Full |
| 26 | MCP.md | mcp-and-comm.md (#8) | Full |
| 27 | PROVIDERS.md | agent-core.md (#13) | Full |

**Result: 27/27 Go SME docs have Rust migration coverage (100%).**

---

## Status Overview

| Category | Go | Rust | Coverage |
|---|---|---|---|
| HTTP Routes | 200+ | 95 | ~47% |
| Source Files | 498 internal | 141 | ~28% |
| DB Migrations | 46 | 47 | 100% |
| DB Query Files | 26 | 16 | ~62% |
| Agent Tools | 118 files (30+ platform) | 15 tools | ~13% |
| Handler Files | 87 (20 categories) | 14 | ~16% |
| WebSocket Endpoints | 4 | 1 (merged) | 25% |
| AI Providers | 5 + CLI | 6 (no CLI/local) | ~75% |
| CLI Commands | 12+ | 9 | ~75% |
| Tests | Extensive | 72 executables | Partial |

---

## Gap Analysis (49 gaps, by severity)

### CRITICAL (9 items) -- Product cannot ship without these

#### 1. Voice System
- **Go:** 22 files -- ASR (Whisper), TTS (Kokoro), VAD (Silero+RMS), wakeword, duplex audio, phonemization, ONNX
- **Rust:** Empty stub (`voice/lib.rs`)
- **Go paths:** `internal/voice/`
- **WS endpoints missing:** `/ws/voice`, `/ws/voice/wake`
- **Handler missing:** `POST /api/v1/voice/transcribe`, `POST /api/v1/voice/tts`, `GET /api/v1/voice/voices`, voice model management

#### 2. App Platform (.napp Runtime)
- **Go:** 44 files -- gRPC adapters, runtime, sandbox, signing, supervisor, watcher, hooks, installer, inspector, 8 protobuf defs
- **Rust:** 9 files -- basic manifest, napp, registry, runtime, sandbox, signing, supervisor, hooks
- **Go paths:** `internal/apps/`
- **Missing:** gRPC adapter, inspector, install from URL, file watcher, per-app settings store, protobuf service impls, app UI routes, app OAuth routes

#### 3. Platform Tools (macOS/Windows/Linux)
- **Go:** 70+ files with build tags
- **Rust:** 0 platform tools
- **Missing tools:**
  - Clipboard (darwin/linux/windows)
  - Contacts (darwin/linux/windows)
  - Calendar (darwin/linux/windows)
  - Mail (darwin/linux/windows)
  - Messages (darwin -- iMessage)
  - Music (darwin/linux/windows)
  - Reminders (darwin/linux/windows)
  - Keychain (darwin/linux/windows)
  - Accessibility (darwin/linux/windows)
  - Spotlight (darwin/linux/windows)
  - Spaces (darwin/windows)
  - Dock (darwin)
  - Menubar (darwin/windows)
  - Shortcuts (darwin/linux/windows)
  - Dialogs (darwin/windows)
  - Window management (darwin/linux/windows)
  - Notifications (darwin/linux/windows)
  - App management (darwin/linux/windows)
  - Desktop operations (darwin/linux/windows)
  - System domain (darwin/linux/windows)
  - PIM domain (darwin/linux/windows)
  - Screenshot, TTS tool, Vision

#### 4. Snapshot System (Computer Use)
- **Go:** Screen capture + accessibility tree + annotation + rendering (platform-specific)
- **Rust:** Browser `snapshot.rs` only covers browser pages, NOT desktop
- **Go paths:** `internal/agent/tools/snapshot_*`
- **Missing:** Desktop screen capture, accessibility tree extraction, annotation overlay, visual rendering pipeline

#### 5. Orchestrator (Sub-Agent Spawning)
- **Go:** `internal/agent/orchestrator/orchestrator.go` -- up to 5 concurrent sub-agents, persisted to `pending_tasks`
- **Rust:** No orchestrator
- **Missing:** Sub-agent spawn/manage/join, pending task persistence, subagent lane, `agent(resource: task, action: spawn)` tool

#### 6. Lane-Based Concurrency
- **Go:** `internal/agenthub/` -- 6 lanes (main/events/subagent/nested/heartbeat/comm) with configurable concurrency limits
- **Rust:** Raw `tokio::spawn()` with no queuing or limits
- **Missing:** Lane type system, serialized main lane, lane-aware routing, lane status reporting
- **Risk:** Without serialized main lane, concurrent user messages can corrupt conversation state

#### 7. Crash Recovery
- **Go:** `internal/agent/recovery/` -- persists sub-agent tasks, recovers on restart
- **Rust:** None
- **Missing:** Pending task persistence, recovery on startup, crash log capture

#### 8. Embeddings / Vector Search
- **Go:** `internal/agent/embeddings/` -- vector storage, hybrid search (vector+FTS), text chunking, embedding providers
- **Rust:** None -- memory is basic DB queries only
- **Missing:** Vector embedding generation, vector storage, hybrid search, text chunking, embedding provider integrations

#### 9. Auth Routes (Account Management)
- **Go:** verify-email, resend-verification, forgot-password, reset-password + email sending service
- **Rust:** Only login, register, refresh
- **Missing routes:**
  - `POST /api/v1/auth/verify-email`
  - `POST /api/v1/auth/resend-verification`
  - `POST /api/v1/auth/forgot-password`
  - `POST /api/v1/auth/reset-password`
- **Missing service:** Email sender

---

### HIGH (14 items) -- Significant feature gaps

#### 10. Browser Relay
- **Go:** `internal/browser/relay.go` -- Chrome extension bridge, CDP proxy
- **Rust:** Browser crate has no relay
- **Missing:** Chrome extension relay WS, CDP proxy, browser audit
- **Go routes missing:**
  - `POST /relay/extension`
  - `POST /relay/cdp`
  - `GET /relay/extension/status`
  - `GET /relay/extension/token`
  - `GET /relay/json/version`, `/json`, `/json/list`, `/json/activate/{id}`, `/json/close/{id}`

#### 11. NeboLoop Comm Plugin
- **Go:** `internal/agent/comm/neboloop/` -- WebSocket gateway client, bot auth, DM handling, channel messaging, auto-reconnect
- **Rust:** Comm crate has loopback only
- **Missing:** NeboLoop WS client, bot registration, DM send/receive, channel messaging, auto-reconnect, owner DM routing

#### 12. MCP Server + OAuth
- **Go:** `internal/mcp/` -- JSON-RPC server, protocol handler, authentication, context, OAuth, MCP-exposed tools
- **Rust:** MCP bridge + client + crypto only, NO server
- **Missing:** MCP JSON-RPC server, MCP auth, MCP context, MCP OAuth handler, MCP-exposed tools (memory, notification, user)
- **Go routes missing:** `/mcp/*`, `/mcp/oauth/*`, `/agent/mcp/*`

#### 13. Plugin System + Store/Marketplace
- **Go:** `internal/handler/plugins/handler.go` (939 lines) + DB tables
- **Rust:** None
- **Missing routes:**
  - `GET/PUT/POST /api/v1/plugins`
  - `GET/POST/DELETE /api/v1/store/apps`, store skills, reviews
- **Missing:** Plugin registry, settings management, toggle, hot-reload, NeboLoop marketplace

#### 14. Developer Routes
- **Go:** `internal/handler/dev/` -- 4 files
- **Rust:** None
- **Missing routes:**
  - `POST /dev/sideload`, `DELETE /dev/sideload/{appId}`
  - `GET /dev/apps`, `POST /dev/apps/{appId}/relaunch`
  - `GET /dev/apps/{appId}/logs`, `GET /dev/apps/{appId}/grpc`, `GET /dev/apps/{appId}/context`
  - `GET /dev/tools`, `POST /dev/tools/execute`
  - `POST /dev/browse-directory`, `POST /dev/open-window`

#### 15. OAuth Broker
- **Go:** `internal/oauth/` -- centralized token management for third-party services
- **Rust:** None
- **Missing:** OAuth URL generation, callback handling, disconnect, provider listing, token refresh

#### 16. Message Bus (Pub/Sub)
- **Go:** `internal/msgbus/` -- in-process pub/sub, topic subscription, message sink
- **Rust:** None -- direct function calls / WS broadcasts only
- **Impact:** Tightly couples components that should be decoupled

#### 17. Events System
- **Go:** `internal/events/` -- typed event topics, emission infrastructure
- **Rust:** No centralized event system
- **Impact:** No event-driven coordination between subsystems

#### 18. Keyring / Credential Storage
- **Go:** `internal/keyring/` + `internal/credential/` -- OS-native keychain integration
- **Rust:** None
- **Missing:** macOS Keychain, Windows Credential Manager, Linux Secret Service, credential migration

#### 19. Real-Time Hub (Dual Hub Architecture)
- **Go:** Separate agent hub (`internal/agenthub/`) + client hub (`internal/realtime/`)
- **Rust:** Single merged hub
- **Missing:** Frame type routing (event/res/stream/req), chat context handler, rewrite handler, agent-initiated request handling

#### 20. Desktop Integration (Wails)
- **Go:** `internal/webview/` -- 12 files for native window management
- **Rust:** Tauri is separate project
- **Missing:** Native window management from server, webview callbacks, cursor management, JS injection
- **Note:** May be N/A if Tauri covers all desktop needs

#### 21. Heartbeat Daemon
- **Go:** `internal/daemon/heartbeat.go` -- background tick runner in heartbeat lane
- **Rust:** Endpoints exist but no daemon runs
- **Missing:** Background heartbeat tick, proactive behavior triggers

#### 22. Notification System
- **Go:** `internal/notify/` -- macOS NSUserNotification, Windows Toast, Linux D-Bus
- **Rust:** Stub crate
- **Missing:** All platform-specific desktop notifications

#### 23. Full Agent Tools
Go tools missing from Rust tool registry:
- `memory.go` -- memory store/recall/search/forget
- `cron.go` + `scheduler.go` + `scheduler_manager.go` -- scheduling
- `loop_tool.go` + `neboloop_tool.go` -- NeboLoop integration
- `query_sessions.go` -- past session querying
- `screenshot.go` -- screenshot capture
- `vision.go` -- image analysis
- `tts.go` -- text-to-speech
- `desktop_queue.go` -- desktop work queue

---

### MEDIUM (16 items)

#### 24. Native Gemini Provider
- Go has native implementation with turn normalization
- Rust proxies through OpenAI compatibility layer

#### 25. Local/CGO AI Provider
- Go has CGO-based local model execution + manager
- Rust has none

#### 26. CLI AI Providers
- Go wraps `claude`/`gemini`/`codex` CLI as providers
- Rust detects CLIs but doesn't use them as providers

#### 27. Markdown Processing
- Go: `internal/markdown/` with tests
- Rust: None

#### 28. Embedded Ripgrep Binaries
- Go: Platform-specific embedded rg binaries
- Rust: Calls system `rg` (no guarantee of availability)

#### 29. Crashlog System
- Go: `internal/crashlog/`
- Rust: None

#### 30. Security Subsystem
- **Missing from Rust:** CSRF middleware, validation middleware, input sanitization, SQL injection prevention, safe encoding

#### 31. Full Memory System
- **Missing:** File-based memory, personality tracking, database context builder

#### 32. Full Steering Pipeline
- **Missing generators:** compactionRecovery, taskProgress, janusQuotaWarning, templates

#### 33. Full Session Management
- **Missing:** Key parser for routing (companion/DM/channel sessions), session policies, compaction tracking

#### 34. AI Provider Features
- **Missing:** Message deduplication (`dedupe.go`), fuzzy model matching (`fuzzy.go`), local model management, process signal handling

#### 35. CLI Commands
- **Missing:** `agent`, `message`, `plugins`, `desktop`, `updates`, `vars`

#### 36. Lifecycle Management
- Go: `internal/lifecycle/lifecycle.go`
- Rust: None

#### 37. App UI Routes
- **Missing:** `GET /api/v1/apps/ui`, `POST /api/v1/apps/{id}/ui/open`, `GET /api/v1/apps/{id}/ui/*`, `FUNC /api/v1/apps/{id}/api/*`

#### 38. OAuth Routes
- **Missing:** `GET /api/v1/oauth/{provider}/url`, `POST /api/v1/oauth/{provider}/callback`, `DELETE /api/v1/oauth/{provider}`, `GET /api/v1/oauth/providers`

#### 39. Protected User Routes
- **Missing (JWT-gated):** `GET /api/v1/user/me`, `PUT /api/v1/user/me`, `DELETE /api/v1/user/me`, `POST /api/v1/user/me/change-password`

---

### LOW (10 items)

#### 40. ~~AFV (Audio/Video Fence)~~ — DROPPED
- **Decision:** AFV is not being migrated to Rust. Removed from scope.

#### 41. Loop/NeboLoop Tools
- Agent tools for NeboLoop channel/loop interaction

#### 42. Web Content Sanitization
- Go: `web_sanitize.go` with tests
- Rust: None

#### 43. Cache Control (ETag)
- Go middleware for ETag-based caching
- Rust: None

#### 44. HTTP Validation Utilities
- Go: Rich `httputil` with typed parsing
- Rust: Uses Axum extractors directly

#### 45. App OAuth Handler
- Go: `internal/handler/appoauth/handler.go`
- Rust: None

#### 46. Store/Marketplace Routes
- Already covered under Plugin System (#13)

#### 47. Database Query Coverage
- Missing query modules for: plugins, dev apps, embeddings, voice models

#### 48. Minor Handler Gaps
- `listagentshandler.go`, `loopshandler.go`, `getsimpleagentstatushandler.go`

#### 49. Logging Handler
- Go: Custom handler in `internal/logging/`
- Rust: Uses standard `tracing`

---

## Route Mapping: Go to Rust

### Legend
- Y = Exists in Rust
- N = Missing from Rust
- P = Partially implemented

### Auth Routes

| Route | Go | Rust |
|---|---|---|
| `GET /api/v1/auth/config` | Y | Y |
| `POST /api/v1/auth/login` | Y | Y |
| `POST /api/v1/auth/register` | Y | Y |
| `POST /api/v1/auth/verify-email` | Y | N |
| `POST /api/v1/auth/resend-verification` | Y | N |
| `POST /api/v1/auth/forgot-password` | Y | N |
| `POST /api/v1/auth/reset-password` | Y | N |
| `POST /api/v1/auth/refresh` | Y | Y |

### Setup Routes

| Route | Go | Rust |
|---|---|---|
| `POST /api/v1/setup/admin` | Y | Y |
| `POST /api/v1/setup/complete` | Y | Y |
| `GET /api/v1/setup/status` | Y | Y |
| `GET /api/v1/setup/personality` | Y | Y |
| `PUT /api/v1/setup/personality` | Y | Y |

### Chat Routes

| Route | Go | Rust |
|---|---|---|
| `GET /api/v1/chats` | Y | Y |
| `POST /api/v1/chats` | Y | Y |
| `GET /api/v1/chats/{id}` | Y | Y |
| `PUT /api/v1/chats/{id}` | Y | Y |
| `DELETE /api/v1/chats/{id}` | Y | Y |
| `GET /api/v1/chats/companion` | Y | Y |
| `GET /api/v1/chats/days` | Y | Y |
| `GET /api/v1/chats/history/{day}` | Y | Y |
| `POST /api/v1/chats/message` | Y | Y |
| `GET /api/v1/chats/search` | Y | Y |

### Agent Routes

| Route | Go | Rust |
|---|---|---|
| `GET /api/v1/agent/sessions` | Y | Y |
| `DELETE /api/v1/agent/sessions/{id}` | Y | Y |
| `GET /api/v1/agent/sessions/{id}/messages` | Y | Y |
| `GET /api/v1/agent/settings` | Y | Y |
| `PUT /api/v1/agent/settings` | Y | Y |
| `GET /api/v1/agent/heartbeat` | Y | Y |
| `PUT /api/v1/agent/heartbeat` | Y | Y |
| `GET /api/v1/agent/advisors` | Y | Y |
| `GET /api/v1/agent/advisors/{name}` | Y | Y |
| `POST /api/v1/agent/advisors` | Y | Y |
| `PUT /api/v1/agent/advisors/{name}` | Y | Y |
| `DELETE /api/v1/agent/advisors/{name}` | Y | Y |
| `GET /api/v1/agent/status` | Y | Y |
| `GET /api/v1/agent/lanes` | Y | P (returns empty) |
| `GET /api/v1/agent/loops` | Y | N |
| `GET /api/v1/agent/channels/{id}/messages` | Y | Y |
| `POST /api/v1/agent/channels/{id}/send` | Y | Y |
| `GET /api/v1/agent/profile` | Y | Y |
| `PUT /api/v1/agent/profile` | Y | Y |
| `GET /api/v1/agent/system-info` | Y | Y |
| `GET /api/v1/agent/personality-presets` | Y | Y |
| `GET /api/v1/agents` | Y | N |
| `GET /api/v1/agents/{id}/status` | Y | N |

### Memory Routes

| Route | Go | Rust |
|---|---|---|
| `GET /api/v1/memories` | Y | Y |
| `GET /api/v1/memories/search` | Y | Y |
| `GET /api/v1/memories/stats` | Y | Y |
| `GET /api/v1/memories/{id}` | Y | Y |
| `PUT /api/v1/memories/{id}` | Y | Y |
| `DELETE /api/v1/memories/{id}` | Y | Y |

### Task/Cron Routes

| Route | Go | Rust |
|---|---|---|
| `GET /api/v1/tasks` | Y | Y |
| `POST /api/v1/tasks` | Y | Y |
| `GET /api/v1/tasks/{name}` | Y | Y |
| `PUT /api/v1/tasks/{name}` | Y | Y |
| `DELETE /api/v1/tasks/{name}` | Y | Y |
| `POST /api/v1/tasks/{name}/toggle` | Y | Y |
| `POST /api/v1/tasks/{name}/run` | Y | Y |
| `GET /api/v1/tasks/{name}/history` | Y | Y |

### Integration (MCP) Routes

| Route | Go | Rust |
|---|---|---|
| `GET /api/v1/integrations` | Y | Y |
| `GET /api/v1/integrations/registry` | Y | Y |
| `GET /api/v1/integrations/tools` | Y | Y |
| `POST /api/v1/integrations` | Y | Y |
| `GET /api/v1/integrations/{id}` | Y | Y |
| `PUT /api/v1/integrations/{id}` | Y | Y |
| `DELETE /api/v1/integrations/{id}` | Y | Y |
| `POST /api/v1/integrations/{id}/test` | Y | Y |
| `GET /api/v1/integrations/{id}/oauth-url` | Y | N |
| `POST /api/v1/integrations/{id}/disconnect` | Y | N |
| `GET /api/v1/integrations/oauth/callback` | Y | N |

### Provider/Model Routes

| Route | Go | Rust |
|---|---|---|
| `GET /api/v1/models` | Y | Y |
| `PUT /api/v1/models/config` | Y | Y |
| `PUT /api/v1/models/{provider}/{modelId}` | Y | Y |
| `PUT /api/v1/models/cli/{cliId}` | Y | Y |
| `PUT /api/v1/models/task-routing` | Y | Y |
| `GET /api/v1/providers` | Y | Y |
| `POST /api/v1/providers` | Y | Y |
| `GET /api/v1/providers/{id}` | Y | Y |
| `PUT /api/v1/providers/{id}` | Y | Y |
| `DELETE /api/v1/providers/{id}` | Y | Y |
| `POST /api/v1/providers/{id}/test` | Y | Y |

### Voice Routes

| Route | Go | Rust |
|---|---|---|
| `POST /api/v1/voice/transcribe` | Y | N |
| `POST /api/v1/voice/tts` | Y | N |
| `GET /api/v1/voice/voices` | Y | N |
| `GET /api/v1/voice/models/status` | Y | N |
| `POST /api/v1/voice/models/download` | Y | N |

### Skill/Extension Routes

| Route | Go | Rust |
|---|---|---|
| `GET /api/v1/extensions` | Y | Y |
| `POST /api/v1/skills` | Y | Y |
| `GET /api/v1/skills/{name}` | Y | Y |
| `GET /api/v1/skills/{name}/content` | Y | Y |
| `PUT /api/v1/skills/{name}` | Y | Y |
| `DELETE /api/v1/skills/{name}` | Y | Y |
| `POST /api/v1/skills/{name}/toggle` | Y | Y |

### File Routes

| Route | Go | Rust |
|---|---|---|
| `POST /api/v1/files/browse` | Y | Y |
| `GET /api/v1/files/*` | Y | Y |

### User Routes

| Route | Go | Rust |
|---|---|---|
| `GET /api/v1/user/me/profile` | Y | Y |
| `PUT /api/v1/user/me/profile` | Y | Y |
| `GET /api/v1/user/me/preferences` | Y | Y |
| `PUT /api/v1/user/me/preferences` | Y | Y |
| `GET /api/v1/user/me/permissions` | Y | Y |
| `PUT /api/v1/user/me/permissions` | Y | Y |
| `POST /api/v1/user/me/accept-terms` | Y | Y |
| `GET /api/v1/user/me` (JWT) | Y | N |
| `PUT /api/v1/user/me` (JWT) | Y | N |
| `DELETE /api/v1/user/me` (JWT) | Y | N |
| `POST /api/v1/user/me/change-password` (JWT) | Y | N |

### Notification Routes

| Route | Go | Rust |
|---|---|---|
| `GET /api/v1/notifications` | Y | Y |
| `DELETE /api/v1/notifications/{id}` | Y | Y |
| `PUT /api/v1/notifications/{id}/read` | Y | Y |
| `PUT /api/v1/notifications/read-all` | Y | Y |
| `GET /api/v1/notifications/unread-count` | Y | Y |

### NeboLoop Routes

| Route | Go | Rust |
|---|---|---|
| `GET /api/v1/neboloop/oauth/start` | Y | Y |
| `GET /api/v1/neboloop/oauth/status` | Y | Y |
| `POST /api/v1/neboloop/register` | Y | N |
| `POST /api/v1/neboloop/login` | Y | N |
| `GET /api/v1/neboloop/account` | Y | Y |
| `DELETE /api/v1/neboloop/account` | Y | Y |
| `GET /api/v1/neboloop/janus/usage` | Y | Y |
| `GET /api/v1/neboloop/open` | Y | Y |
| `POST /api/v1/neboloop/connect` | Y | N |
| `GET /api/v1/neboloop/status` | Y | Y |

### OAuth Routes

| Route | Go | Rust |
|---|---|---|
| `GET /api/v1/oauth/{provider}/url` | Y | N |
| `POST /api/v1/oauth/{provider}/callback` | Y | N |
| `DELETE /api/v1/oauth/{provider}` (JWT) | Y | N |
| `GET /api/v1/oauth/providers` (JWT) | Y | N |

### Plugin Routes

| Route | Go | Rust |
|---|---|---|
| `GET /api/v1/plugins` | Y | N |
| `GET /api/v1/plugins/{id}` | Y | N |
| `PUT /api/v1/plugins/{id}/settings` | Y | N |
| `PUT /api/v1/plugins/{id}/toggle` | Y | N |

### Store Routes

| Route | Go | Rust |
|---|---|---|
| `GET /api/v1/store/apps` | Y | N |
| `GET /api/v1/store/apps/{id}` | Y | N |
| `GET /api/v1/store/apps/{id}/reviews` | Y | N |
| `POST /api/v1/store/apps/{id}/install` | Y | N |
| `DELETE /api/v1/store/apps/{id}/install` | Y | N |
| `GET /api/v1/store/skills` | Y | N |
| `POST /api/v1/store/skills/{id}/install` | Y | N |
| `DELETE /api/v1/store/skills/{id}/install` | Y | N |

### Update Routes

| Route | Go | Rust |
|---|---|---|
| `GET /api/v1/update/check` | Y | Y |
| `POST /api/v1/update/apply` | Y | Y |

### Dev Routes

| Route | Go | Rust |
|---|---|---|
| `POST /dev/sideload` | Y | N |
| `DELETE /dev/sideload/{appId}` | Y | N |
| `GET /dev/apps` | Y | N |
| `POST /dev/apps/{appId}/relaunch` | Y | N |
| `GET /dev/apps/{appId}/logs` | Y | N |
| `GET /dev/apps/{appId}/grpc` | Y | N |
| `GET /dev/apps/{appId}/context` | Y | N |
| `GET /dev/tools` | Y | N |
| `POST /dev/tools/execute` | Y | N |
| `POST /dev/browse-directory` | Y | N |
| `POST /dev/open-window` | Y | N |

### App UI Routes

| Route | Go | Rust |
|---|---|---|
| `GET /api/v1/apps/ui` | Y | N |
| `POST /api/v1/apps/{id}/ui/open` | Y | N |
| `GET /api/v1/apps/{id}/ui/*` | Y | N |
| `FUNC /api/v1/apps/{id}/api/*` | Y | N |
| `GET /api/v1/apps/{appId}/oauth/*` | Y | N |
| `GET /api/v1/apps/oauth/callback` | Y | N |

### WebSocket Routes

| Route | Go | Rust |
|---|---|---|
| `GET /ws` | Y | Y |
| `GET /api/v1/agent/ws` | Y | Y (merged into /ws) |
| `GET /ws/voice` | Y | N |
| `GET /ws/voice/wake` | Y | N |

### Relay Routes

| Route | Go | Rust |
|---|---|---|
| `POST /relay/extension` | Y | N |
| `POST /relay/cdp` | Y | N |
| `GET /relay/extension/status` | Y | N |
| `GET /relay/extension/token` | Y | N |
| `GET /relay/json/*` | Y | N |

### MCP Protocol Routes

| Route | Go | Rust |
|---|---|---|
| `/mcp/*` | Y | N |
| `/mcp/oauth/*` | Y | N |
| `/agent/mcp/*` | Y | N |

### External Routes

| Route | Go | Rust |
|---|---|---|
| `GET /auth/neboloop/callback` | Y | Y |
| `POST /internal/webview/callback` | Y | N |

---

## Tool Mapping: Go to Rust

### Agent Tools

| Tool | Go File | Rust File | Status |
|---|---|---|---|
| Shell/Bash | `shell_tool.go` | `shell_tool.rs` | Y |
| File (read/write/edit) | `file_tool.go` | `file_tool.rs` | Y |
| Grep/Glob | `file_tool.go` | `grep_tool.rs` | Y |
| Web (fetch/search/navigate) | `web_tool.go` | `web_tool.rs` | Y |
| System info | `system_tool.go` | `system_tool.rs` | Y |
| Skill execution | (in runner) | `skill_tool.rs` | Y |
| Message/Comm | `message_tool.go` | `message_tool.rs` | Y |
| Bot control | `agent_tool.go` | `bot_tool.rs` | Y |
| Event trigger | (in agent_tool) | `event_tool.rs` | Y |
| Memory | `memory.go` | -- | N |
| Cron/Scheduler | `cron.go` + `scheduler.go` | -- | N |
| Screenshot | `screenshot.go` | -- | N |
| Vision | `vision.go` | -- | N |
| TTS | `tts.go` | -- | N |
| Desktop queue | `desktop_queue.go` | -- | N |
| Loop/NeboLoop | `loop_tool.go` + `neboloop_tool.go` | -- | N |
| Query sessions | `query_sessions.go` | -- | N |
| Web sanitize | `web_sanitize.go` | -- | N |

### Platform Tools (ALL missing from Rust)

| Tool | macOS | Linux | Windows |
|---|---|---|---|
| Clipboard | `clipboard_darwin.go` | `clipboard_linux.go` | `clipboard_windows.go` |
| Contacts | `contacts_darwin.go` | `contacts_linux.go` | `contacts_windows.go` |
| Calendar | `calendar_darwin.go` | `calendar_linux.go` | `calendar_windows.go` |
| Mail | `mail_darwin.go` | `mail_linux.go` | `mail_windows.go` |
| Messages | `messages_darwin.go` | -- | -- |
| Music | `music_darwin.go` | `music_linux.go` | `music_windows.go` |
| Reminders | `reminders_darwin.go` | `reminders_linux.go` | `reminders_windows.go` |
| Keychain | `keychain_darwin.go` | `keychain_linux.go` | `keychain_windows.go` |
| Accessibility | `accessibility_darwin.go` | `accessibility_linux.go` | `accessibility_windows.go` |
| Spotlight | `spotlight_darwin.go` | `spotlight_linux.go` | `spotlight_windows.go` |
| Spaces | `spaces_darwin.go` | -- | `spaces_windows.go` |
| Dock | `dock_darwin.go` | -- | -- |
| Menubar | `menubar_darwin.go` | -- | `menubar_windows.go` |
| Shortcuts | `shortcuts_darwin.go` | `shortcuts_linux.go` | `shortcuts_windows.go` |
| Dialogs | `dialog_darwin.go` | -- | `dialog_windows.go` |
| Window mgmt | `window_darwin.go` | `window_linux.go` | `window_windows.go` |
| Notifications | `notification_darwin.go` | `notification_linux.go` | `notification_windows.go` |
| App mgmt | `app_darwin.go` | `app_linux.go` | `app_windows.go` |
| Desktop ops | `desktop_darwin.go` | `desktop_linux.go` | `desktop_windows.go` |
| System domain | `system_domain_darwin.go` | `system_domain_linux.go` | `system_domain_windows.go` |
| PIM domain | `pim_domain_darwin.go` | `pim_domain_linux.go` | `pim_domain_windows.go` |
| Snapshot capture | `snapshot_capture_darwin.go` | `snapshot_capture_linux.go` | `snapshot_capture_windows.go` |
| Snapshot a11y | `snapshot_accessibility_darwin.go` | `snapshot_accessibility_linux.go` | `snapshot_accessibility_windows.go` |
| Desktop domain | `desktop_domain_darwin.go` | `desktop_domain_linux.go` | `desktop_domain_windows.go` |

---

## Middleware Mapping

| Middleware | Go | Rust |
|---|---|---|
| JWT Auth | Y | Y |
| Rate Limiting | Y | Y |
| CORS | Y | Y |
| Security Headers | Y | Y |
| Compression | Y | Y |
| CSRF | Y | N |
| Validation | Y | N |
| Cache Control (ETag) | Y | N |

---

## Recommended Migration Phases

### Phase 1: Core Agent Parity (Critical)
1. Lane-based concurrency system
2. Orchestrator (sub-agent spawning)
3. Crash recovery
4. Embeddings / vector search
5. Auth routes (email verify, password reset)
6. Full memory system + tools

### Phase 2: Desktop Agent Experience (Critical)
7. Platform tools (macOS first: clipboard, notifications, window mgmt)
8. Snapshot system (screen capture + accessibility tree)
9. Voice pipeline (ASR, TTS, VAD, wakeword)

### Phase 3: Ecosystem (High)
10. NeboLoop comm plugin
11. MCP server + OAuth
12. Plugin system + store/marketplace
13. OAuth broker
14. Browser relay
15. Developer routes
16. Message bus + events system
17. Keyring/credential storage
18. Real-time hub (dual architecture)

### Phase 4: Feature Parity (Medium)
19. CLI providers
20. Local/CGO AI provider
21. Native Gemini provider
22. Full steering pipeline
23. Full session management
24. Remaining middleware (CSRF, validation, cache)
25. Ripgrep embedding
26. Missing CLI commands
27. App platform full (gRPC, inspector, watcher)

### Phase 5: Polish (Low)
28. ~~AFV system~~ — DROPPED
29. Markdown processing
30. Crashlog
31. Web sanitization
32. Minor handler gaps
33. Database query coverage

---

## File Reference Quick Links

### Go Source (read these when implementing Rust equivalent)

| Component | Go Path |
|---|---|
| Server routes | `internal/server/server.go` |
| Service context | `internal/svc/servicecontext.go` |
| Agent hub | `internal/agenthub/hub.go` |
| Lane system | `internal/agenthub/lane.go` |
| Agent runner | `internal/agent/runner/runner.go` |
| Tool registry | `internal/agent/tools/registry.go` |
| Orchestrator | `internal/agent/orchestrator/orchestrator.go` |
| Crash recovery | `internal/agent/recovery/recovery.go` |
| Embeddings | `internal/agent/embeddings/` |
| Memory | `internal/agent/memory/` |
| Steering | `internal/agent/steering/` |
| Session | `internal/agent/session/` |
| Skills | `internal/agent/skills/` |
| Voice | `internal/voice/` |
| Browser relay | `internal/browser/relay.go` |
| MCP server | `internal/mcp/server.go` |
| Message bus | `internal/msgbus/` |
| Events | `internal/events/` |
| Keyring | `internal/keyring/` |
| OAuth broker | `internal/oauth/` |
| Plugin handler | `internal/handler/plugins/handler.go` |
| Dev handler | `internal/handler/dev/` |
| Auth handler | `internal/handler/auth/` |
| Webview | `internal/webview/` |
| Heartbeat daemon | `internal/daemon/heartbeat.go` |
| DB migrations | `internal/db/migrations/` |
| DB queries | `internal/db/queries/` |
| Middleware | `internal/middleware/` |
| Config | `internal/config/` |
| Root cmd | `cmd/nebo/root.go` |
| Agent cmd | `cmd/nebo/agent.go` |

### Rust Target

| Component | Rust Path |
|---|---|
| Server routes | `crates/server/src/lib.rs` |
| App state | `crates/server/src/state.rs` |
| WS handler | `crates/server/src/handlers/ws.rs` |
| Agent runner | `crates/agent/src/runner.rs` |
| Tool registry | `crates/tools/src/registry.rs` |
| AI providers | `crates/ai/src/providers/` |
| DB store | `crates/db/src/store.rs` |
| DB models | `crates/db/src/models.rs` |
| Auth service | `crates/auth/src/service.rs` |
| Config | `crates/config/src/config.rs` |
| MCP bridge | `crates/mcp/src/bridge.rs` |
| Browser | `crates/browser/src/` |
| Apps | `crates/apps/src/` |
| Tools | `crates/tools/src/` |
| CLI | `crates/cli/src/main.rs` |
| Scheduler | `crates/server/src/scheduler.rs` |
| Updater | `crates/updater/src/` |

---

*Generated: 2026-03-04*
*Last updated by: Migration SME audit*
