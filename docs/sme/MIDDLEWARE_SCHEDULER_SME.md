# Middleware & Scheduler System -- SME Deep-Dive

> **Last updated:** 2026-05-15
>
> **Purpose:** Definitive technical reference for Nebo's HTTP middleware stack (security headers, CORS, authentication, rate limiting, compression, tracing) and its background scheduler systems (cron jobs, heartbeats, memory consolidation, resource monitoring, progress broadcasting). Covers architecture, layer ordering, configuration, key structs, and cross-system interactions.

---

## Key Files

| File | Purpose | Status |
|------|---------|--------|
| `crates/server/src/middleware.rs` | JWT auth, security headers, rate limiter, MCP API key auth | Active |
| `crates/server/src/lib.rs` | Router composition, middleware layering, scheduler spawning, CORS config | Active |
| `crates/server/src/scheduler.rs` | Cron job scheduler (shell, agent, workflow tasks) | Active |
| `crates/server/src/heartbeat.rs` | Heartbeat scheduler for per-entity proactive prompts | Active |
| `crates/server/src/run_registry.rs` | Global active run registry with visibility and cancellation | Active |
| `crates/server/src/workflow_manager.rs` | Workflow lifecycle and execution bridge | Active |
| `crates/server/src/state.rs` | `AppState` definition (shared across all handlers) | Active |
| `crates/server/src/routes/mod.rs` | API route composition, auth/public/protected grouping | Active |
| `crates/server/src/routes/auth.rs` | Auth route definitions (login, register, refresh, etc.) | Active |
| `crates/server/src/spa.rs` | SPA handler, embedded frontend, cache control | Active |
| `crates/server/src/entity_config.rs` | Entity config resolution (heartbeat interval inheritance) | Active |
| `crates/agent/src/memory_consolidation.rs` | Background memory dedup/merge/prune sweep | Active |
| `crates/agent/src/concurrency.rs` | Adaptive concurrency controller + resource monitor | Active |

---

## Part 1: Middleware System

### 1.1 Architecture Overview

Nebo uses Axum 0.8 with Tower middleware layers. The middleware stack is composed in
`crates/server/src/lib.rs::run()` when building the final `Router`. Layers are applied
outside-in: the outermost layer processes the request first (on the way in) and the
response last (on the way out).

```
 Client Request
       |
       v
+----------------------------------------------+
|  TraceLayer (tower_http)                      |  <-- Outermost: request tracing/logging
|  +----------------------------------------+  |
|  |  CorsLayer (tower_http)                |  |  <-- CORS preflight + headers
|  |  +----------------------------------+  |  |
|  |  |  security_headers middleware     |  |  |  <-- HSTS, X-Frame-Options, etc.
|  |  |  +----------------------------+  |  |  |
|  |  |  |  Router                    |  |  |  |
|  |  |  |  +----------------------+  |  |  |  |
|  |  |  |  | WS routes (no comp) |  |  |  |  |  <-- /ws, /ws/app/*, /ws/extension
|  |  |  |  +----------------------+  |  |  |  |
|  |  |  |  | http_routes          |  |  |  |  |
|  |  |  |  | +------------------+ |  |  |  |  |
|  |  |  |  | | CompressionLayer | |  |  |  |  |  <-- gzip/br/deflate for HTTP only
|  |  |  |  | | +--------------+ | |  |  |  |  |
|  |  |  |  | | | /health      | | |  |  |  |  |
|  |  |  |  | | | /server.json | | |  |  |  |  |
|  |  |  |  | | | /agent/mcp   | | |  |  |  |  |  <-- mcp_api_key_auth layer
|  |  |  |  | | | /api/v1/*    | | |  |  |  |  |  <-- api_security_headers layer
|  |  |  |  | | |   +--------+ | | |  |  |  |  |
|  |  |  |  | | |   | auth   | | | |  |  |  |  |  <-- rate_limit + jwt_auth
|  |  |  |  | | |   | public | | | |  |  |  |  |  <-- no auth required
|  |  |  |  | | |   | prot.  | | | |  |  |  |  |  <-- jwt_auth required
|  |  |  |  | | |   +--------+ | | |  |  |  |  |
|  |  |  |  | | | SPA fallback | | |  |  |  |  |
|  |  |  |  | | +--------------+ | |  |  |  |  |
|  |  |  |  | +------------------+ |  |  |  |  |
|  |  |  |  +----------------------+  |  |  |  |
|  |  |  +----------------------------+  |  |  |
|  |  +----------------------------------+  |  |
|  +----------------------------------------+  |
+----------------------------------------------+
       |
       v
 Client Response
```

### 1.2 Middleware Stack (Layer Ordering)

The layers are applied in reverse declaration order (Axum/Tower convention: last
`.layer()` call runs first on request ingress). Here is the exact ordering from
`lib.rs` lines 1782-1802:

```rust
let app = Router::new()
    // 1. WebSocket routes (outside CompressionLayer)
    .route("/ws", ...)
    .route("/ws/app/{agent_id}", ...)
    .route("/ws/extension", ...)
    .route("/ws/voice/dictation", ...)
    .route("/ws/voice/conversation", ...)
    // 2. Static app routes (outside CompressionLayer)
    .route("/apps/{agent_id}/ui/{*path}", ...)
    .route("/sdk/nebo.global.js", ...)
    // 3. Merge HTTP routes (with CompressionLayer)
    .merge(http_routes)
    // 4. Global middleware layers (applied to ALL routes including WS)
    .layer(middleware::security_headers)   // L1: security response headers
    .layer(cors_layer())                   // L2: CORS
    .layer(TraceLayer::new_for_http())     // L3: request tracing
    .with_state(state);
```

**Request processing order (outside-in):**

| Order | Layer | Scope | Purpose |
|-------|-------|-------|---------|
| 1 | `TraceLayer` | All routes | Request/response tracing with method + URI spans |
| 2 | `CorsLayer` | All routes | CORS preflight handling + response headers |
| 3 | `security_headers` | All routes | HSTS, X-Frame-Options, X-Content-Type-Options, etc. |
| 4 | `CompressionLayer` | HTTP routes only | gzip/brotli/deflate response compression |
| 5 | `api_security_headers` | `/api/v1/*` only | CSP `default-src 'none'`, no-cache headers |
| 6 | `mcp_api_key_auth` | `/agent/mcp` only | Opt-in API key validation |
| 7 | `rate_limit` | `/api/v1/auth/*` only | IP-based sliding window rate limiter |
| 8 | `jwt_auth` | Protected routes only | JWT Bearer token validation |

**Design decision -- WebSocket exclusion from compression:**
WebSocket routes are merged directly into the top-level router, outside the
`http_routes` sub-router that carries the `CompressionLayer`. This is intentional:
compression corrupts the upgraded WebSocket connection because the layer wraps the
response body stream, breaking the binary frame protocol.

### 1.3 CORS Configuration

Defined in `lib.rs::cors_layer()` (line 2733):

```rust
fn cors_layer() -> CorsLayer {
    let static_origins = [
        "http://localhost:27895",     // Production server
        "http://127.0.0.1:27895",     // Production (IP)
        "http://localhost:5173",       // Vite dev server
        "http://127.0.0.1:5173",      // Vite dev (IP)
        "http://localhost:4173",       // Vite preview
        "http://127.0.0.1:4173",      // Vite preview (IP)
    ];

    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin, _| {
            // Dynamic: allow neboapp:// origins (Tauri custom protocol)
            if origin.starts_with("neboapp://") {
                return true;
            }
            // Static: allow known localhost origins
            static_origins.contains(origin)
        }))
        .allow_methods([GET, POST, PUT, DELETE, OPTIONS, PATCH])
        .allow_headers(AllowHeaders::mirror_request())
        .allow_credentials(true)
}
```

| Setting | Value | Rationale |
|---------|-------|-----------|
| **Origins** | 6 static localhost + dynamic `neboapp://*` | Localhost-only by design; Tauri uses custom protocol |
| **Methods** | GET, POST, PUT, DELETE, OPTIONS, PATCH | Full REST + CORS preflight |
| **Headers** | `mirror_request()` | Reflects `Access-Control-Request-Headers` back; allows any header the client sends |
| **Credentials** | `true` | Allows cookies/auth headers in cross-origin requests |

**Security note:** No wildcard origins. The server blocks non-loopback binding by default
(line 1814). `NEBO_ALLOW_REMOTE=true` is required to bind to non-localhost addresses,
and a warning is printed if `NEBO_MCP_API_KEY` is unset in that case.

### 1.4 Security Headers Middleware

**File:** `crates/server/src/middleware.rs::security_headers()` (line 77)

Applied to ALL routes (including WebSocket upgrades and SPA). Adds defensive HTTP headers:

| Header | Value | Notes |
|--------|-------|-------|
| `Permissions-Policy` | `accelerometer=(), camera=(self), geolocation=(), gyroscope=(), magnetometer=(), microphone=(self), payment=(), usb=()` | Camera + microphone allowed for self (voice pipeline) |
| `Strict-Transport-Security` | `max-age=31536000; includeSubDomains; preload` | 1-year HSTS (meaningful only if served over TLS via reverse proxy) |
| `X-Content-Type-Options` | `nosniff` | Prevents MIME-type sniffing |
| `X-Frame-Options` | `DENY` | Blocks iframe embedding (except `/chat-embed/*` routes) |
| `X-XSS-Protection` | `1; mode=block` | Legacy XSS filter |
| `Referrer-Policy` | `strict-origin-when-cross-origin` | Limits referrer leakage |

**Embed exception:** If the request path starts with `/chat-embed/`, the `X-Frame-Options`
header is omitted to allow the chat widget to be embedded in iframes on third-party sites.

### 1.5 API Security Headers

**File:** `crates/server/src/middleware.rs::api_security_headers()` (line 107)

Applied only to `/api/v1/*` routes (nested via `routes::api_routes()`):

| Header | Value |
|--------|-------|
| `Content-Security-Policy` | `default-src 'none'; frame-ancestors 'none'` |
| `Cache-Control` | `no-store, no-cache, must-revalidate, private` |
| `Pragma` | `no-cache` |

This is a strict CSP that blocks all content loading -- API responses should never render
HTML or scripts. Cache headers prevent browser/proxy caching of API responses.

### 1.6 JWT Authentication Middleware

**File:** `crates/server/src/middleware.rs::jwt_auth()` (line 23)

```
Request
  |
  +--> Extract JwtSecret from request extensions
  +--> Parse Authorization header ("Bearer <token>")
  +--> Validate via auth::validate_jwt_claims(token, secret)
  |
  +--[OK]--> Insert AuthClaims { user_id, email } into extensions
  |          Continue to next handler
  |
  +--[FAIL]--> 401 JSON { "error": "..." }
```

**Key structs:**

```rust
/// Claims extracted from a validated JWT, stored in request extensions.
pub struct AuthClaims {
    pub user_id: String,
    pub email: String,
}

/// Wrapper type for the JWT secret, stored in request extensions.
pub struct JwtSecret(pub String);
```

**Route application:** JWT auth is only applied to the `protected` router group in
`routes/mod.rs`. The `JwtSecret` is injected as an `Extension` layer alongside the
`jwt_auth` middleware:

```rust
let protected = user::protected_routes()
    .layer(axum::Extension(jwt_secret))
    .layer(axum::middleware::from_fn(middleware::jwt_auth));
```

Currently, only `user::protected_routes()` requires JWT. All other routes (chat, agents,
memory, settings, etc.) are public -- Nebo is designed for localhost-only access where
the owner is the only user.

### 1.7 MCP API Key Authentication

**File:** `crates/server/src/middleware.rs::mcp_api_key_auth()` (line 129)

Opt-in API key authentication for the `/agent/mcp` endpoint. Behavior:

- If `NEBO_MCP_API_KEY` env var is not set or empty: **no auth** (zero-config localhost)
- If set: requires `Authorization: Bearer <key>` matching the env var
- Error responses use JSON-RPC format (for MCP client compatibility):

```json
{
    "jsonrpc": "2.0",
    "id": null,
    "error": {
        "code": -32000,
        "message": "MCP API key required (set NEBO_MCP_API_KEY)"
    }
}
```

### 1.8 Rate Limiting

**File:** `crates/server/src/middleware.rs` (lines 176-237)

In-memory sliding window rate limiter. Applied only to auth routes (login, register,
refresh, etc.) to prevent brute-force attacks.

```
+-------------------+     +-------------------+
| rate_limit()      |     | RateLimiter       |
| (middleware fn)   |---->| {                 |
|                   |     |   buckets: Map<   |
| Extract IP from   |     |     IP -> (count, |
| ConnectInfo only  |     |     window_start) |
| (ignores          |     |   >,              |
|  X-Forwarded-For) |     |   max_requests,   |
|                   |     |   window           |
+-------------------+     | }                 |
                          +-------------------+
```

**Configuration (hardcoded in `routes/mod.rs`):**

```rust
let auth_limiter = RateLimiter::new(
    10,                                    // max_requests: 10
    std::time::Duration::from_secs(60),    // window: 60 seconds
);
```

| Parameter | Value | Description |
|-----------|-------|-------------|
| `max_requests` | 10 | Max requests per window per IP |
| `window` | 60s | Sliding window duration |
| **IP source** | `ConnectInfo<SocketAddr>` only | Peer address from TCP socket |

**Security design decision:** X-Forwarded-For is intentionally ignored because it is
trivially spoofable by any client. The comment notes: "For deployments behind a trusted
reverse proxy, add a TrustedProxy variant." This matches the Go predecessor's
`DefaultKeyFunc` behavior.

**Algorithm:**
1. Extract client IP from `ConnectInfo` (falls back to `127.0.0.1`)
2. Lock the shared `HashMap<IpAddr, (u32, Instant)>`
3. If window has expired (elapsed >= window duration), reset counter
4. Increment counter
5. If counter > max_requests, return `429 Too Many Requests`
6. Otherwise, continue to the next handler

**Struct definition:**

```rust
pub struct RateLimiter {
    buckets: Arc<Mutex<HashMap<IpAddr, (u32, Instant)>>>,
    max_requests: u32,
    window: std::time::Duration,
}
```

The `Mutex` is `tokio::sync::Mutex` (async-aware). The buckets map is never cleaned up
(entries persist for the lifetime of the server). In practice this is not a concern since
Nebo runs locally with very few distinct IPs.

### 1.9 Request Tracing

**File:** `crates/server/src/lib.rs` (lines 1793-1801)

Uses `tower_http::trace::TraceLayer` for structured request logging via `tracing`:

```rust
TraceLayer::new_for_http()
    .make_span_with(|request: &Request<_>| {
        tracing::info_span!("http",
            method = %request.method(),
            uri = %request.uri()
        )
    })
    .on_failure(|error, latency, _span| {
        tracing::error!(
            %error,
            latency_ms = latency.as_millis(),
            "request failed"
        );
    })
```

Every HTTP request creates a tracing span with method and URI. Server errors (5xx) are
logged at ERROR level with latency. Successful requests rely on the span's natural
lifecycle for trace output.

### 1.10 Response Compression

**File:** `crates/server/src/lib.rs` (line 1780)

```rust
.layer(CompressionLayer::new())
```

Uses `tower_http::compression::CompressionLayer` with default settings:
- Supports gzip, brotli, and deflate
- Content negotiation via `Accept-Encoding` header
- Applied to all HTTP routes (not WebSocket routes)
- Minimum body size and content-type filtering use tower-http defaults

### 1.11 SPA Fallback and Cache Control

**File:** `crates/server/src/spa.rs`

The SPA handler serves the embedded frontend (compiled via `rust_embed`) with intelligent
cache control:

| Path Pattern | Cache-Control | Rationale |
|--------------|---------------|-----------|
| `_app/immutable/*` | `public, max-age=31536000, immutable` | Hashed SvelteKit assets (never change) |
| `index.html` / `200.html` | `no-cache` | SPA entry point (must revalidate) |
| Everything else | `public, max-age=3600` | Static assets (1-hour cache) |

The fallback chain: exact file match -> `index.html` -> `200.html` -> 404. This supports
both SvelteKit prerendered pages and the adapter-static fallback.

### 1.12 Route Groups and Auth Boundaries

**File:** `crates/server/src/routes/mod.rs`

```
/api/v1/
  |
  +-- auth_routes (rate limited: 10 req/min)
  |     POST /auth/login
  |     POST /auth/register
  |     POST /auth/refresh
  |     POST /auth/forgot
  |     POST /auth/reset
  |     POST /auth/verify
  |     POST /auth/resend
  |
  +-- public (no auth)
  |     GET  /auth/config
  |     ...chat, agent, memory, provider, skills, tasks,
  |        integrations, files, neboloop, workflows, roles,
  |        commander, plugins, store, entity_config,
  |        notifications, voice, apps, user, codes, deps,
  |        runs/active
  |
  +-- protected (JWT required)
        user::protected_routes()
```

**Layering for api_security_headers:**
```rust
.nest("/api/v1",
    routes::api_routes(jwt_secret)
        .layer(axum::middleware::from_fn(middleware::api_security_headers))
)
```

All `/api/v1/*` responses get the strict CSP and no-cache headers.

### 1.13 Remote Access Guard

**File:** `crates/server/src/lib.rs` (lines 1813-1831)

Before binding the listener, the server checks the host address:

```
if host != "127.0.0.1" && host != "localhost" && host != "::1":
    if NEBO_ALLOW_REMOTE != "true":
        ERROR: "Refusing to bind ... Nebo is designed for localhost-only access"
    else:
        WARNING: "remote access enabled"
        if NEBO_MCP_API_KEY is empty:
            WARNING: "MCP endpoint is UNAUTHENTICATED"
```

This is a defense-in-depth measure that prevents accidental network exposure.

---

## Part 2: Scheduler System

### 2.1 Architecture Overview

Nebo runs multiple independent background scheduler loops, all spawned as Tokio tasks
from `lib.rs::run()`. None use external job frameworks -- they are simple
`tokio::time::interval` loops with domain-specific logic.

```
+-------------------------------------------------------------------+
|                    Server Startup (lib.rs::run())                  |
+-------------------------------------------------------------------+
       |               |               |              |           |
       v               v               v              v           v
+------------+  +------------+  +------------+  +-----------+ +----------+
| Cron       |  | Heartbeat  |  | Memory     |  | Resource  | | Progress |
| Scheduler  |  | Scheduler  |  | Consolidat.|  | Monitor   | | Bcast    |
| (60s tick) |  | (60s tick) |  | (30m tick) |  | (30s tick)| | (5s tick)|
+------------+  +------------+  +------------+  +-----------+ +----------+
   |                |                |               |             |
   v                v                v               v             v
 DB cron_jobs    Entity configs    Memory store    sysinfo      RunRegistry
 Shell/Agent/    Chat dispatch     LLM dedup       CPU/RAM      -> ClientHub
 Workflow exec   per entity        + merge         adaptive       WS bcast
                                                   concurrency
```

Additionally, several one-shot and persistent background tasks are spawned:

```
+-------------------------------------------------------------------+
|              Other Background Tasks (spawned at startup)           |
+-------------------------------------------------------------------+
| - Tool supervisor (15s health check for napp processes)           |
| - NeboLoop auto-connect + reconnect watcher (exp. backoff)       |
| - Background update checker (1h interval, release builds only)   |
| - Skill manifest verification (one-shot at startup)              |
| - MCP integration reconnection (one-shot at startup)             |
| - Tool discovery and launch (one-shot at startup)                |
| - Filesystem watchers (agents, plugins, skills -- continuous)    |
| - Event dispatcher loop (event bus -> role subscriptions)        |
| - Snapshot store cleanup (piggybacks on cron scheduler tick)     |
+-------------------------------------------------------------------+
```

### 2.2 Cron Scheduler

**File:** `crates/server/src/scheduler.rs`

The cron scheduler is the primary job execution engine. It polls the `cron_jobs` table
every 60 seconds and executes any jobs whose cron expression indicates they are due.

#### 2.2.1 Lifecycle

```
spawn()
  |
  +-> 10s initial delay (let server boot)
  |
  +-> loop {
        interval.tick() (60s)
        |
        +-> tick()
        |     +-> delete_completed_tasks() (7-day TTL cleanup)
        |     +-> list_enabled_cron_jobs()
        |     +-> for each job:
        |           +-> normalize_cron()
        |           +-> parse schedule
        |           +-> check if due (last_run vs next occurrence)
        |           +-> execute based on task_type
        |           +-> update last_run + history
        |           +-> send desktop notification
        |
        +-> snapshot_store.cleanup() (expired browser snapshots)
      }
```

#### 2.2.2 Function Signatures

```rust
/// Entry point: spawns the cron scheduler loop as a Tokio task.
pub fn spawn(
    store: Arc<Store>,
    runner: Arc<Runner>,
    hub: Arc<ClientHub>,
    snapshot_store: Arc<browser::SnapshotStore>,
    workflow_manager: Arc<dyn tools::workflows::WorkflowManager>,
    run_registry: RunRegistry,
);

/// Single tick: processes all due jobs.
async fn tick(
    store: &Store,
    runner: &Runner,
    hub: &ClientHub,
    workflow_manager: &dyn tools::workflows::WorkflowManager,
    run_registry: &RunRegistry,
) -> Result<(), String>;
```

#### 2.2.3 Job Types

| `task_type` | Handler | Description |
|-------------|---------|-------------|
| `bash` / `shell` / `""` | `execute_shell()` | Runs command via `sh -c` |
| `agent` | `execute_agent()` | Sends prompt to AI agent runner |
| `workflow` | `execute_workflow_task()` | Triggers a standalone workflow by ID |
| `agent_workflow` / `role_workflow` | `execute_agent_workflow_task()` | Triggers an agent's inline workflow binding |

#### 2.2.4 Shell Execution

```rust
async fn execute_shell(command: &str) -> (bool, String, Option<String>) {
    Command::new("sh").arg("-c").arg(command).output().await
    // Returns (success, stdout, stderr_or_error)
}
```

Simple process spawning via `tokio::process::Command`. No sandboxing, no timeout.
Exit code determines success/failure.

#### 2.2.5 Agent Execution

```rust
async fn execute_agent(
    runner: &Runner,
    hub: &ClientHub,
    job: &db::models::CronJob,
    run_registry: &RunRegistry,
) -> (bool, String, Option<String>);
```

Agent execution flow:

```
1. Build prompt from job.message (fallback: job.command)
2. Create session_key: "cron-{job.name}"
3. Create CancellationToken
4. Register in RunRegistry (visible + cancellable)
5. Build RunRequest { session_key, prompt, system, origin: System, channel: "cron" }
6. Call runner.run(req)
7. Stream events:
   - Text -> accumulate + broadcast via ClientHub
   - Error -> return failure
   - Done -> break
8. Drop RunHandle (auto-unregister)
```

The run appears in the global RunRegistry, making cron runs visible in the UI and
cancellable via the `/api/v1/runs/active` endpoint.

#### 2.2.6 Workflow Execution

```rust
async fn execute_workflow_task(
    manager: &dyn WorkflowManager,
    workflow_id: &str,
) -> (bool, String, Option<String>);
```

Delegates to `WorkflowManager::run(workflow_id, Null, "cron")`. Returns the run_id
on success.

#### 2.2.7 Agent Workflow Execution

```rust
async fn execute_agent_workflow_task(
    manager: &dyn WorkflowManager,
    store: &Store,
    command: &str,  // format: "agent:{agent_id}:{binding_name}"
) -> (bool, String, Option<String>);
```

Flow:

```
1. Parse command -> (agent_id, binding_name)
2. Guard: store.is_agent_workflow_active(agent_id, binding_name)
   - false -> skip ("automation is disabled")
   - error -> fail closed (don't execute)
3. Load agent record from DB
4. Parse agent config from frontmatter
5. Resolve workflow binding
6. Convert binding to workflow JSON
7. Call manager.run_inline(def_json, inputs, "schedule", ...)
```

#### 2.2.8 Cron Schedule Resolution

```
For each job:
  1. Normalize schedule (PersonaTool::normalize_cron handles stale 5-field expressions)
  2. Parse with `cron::Schedule`
  3. Get last_run timestamp:
     - Try parse as i64 (Unix epoch seconds)
     - Try parse as "%Y-%m-%d %H:%M:%S" datetime
     - Default: now (new jobs wait for next occurrence)
  4. Compute next occurrence after last_run
  5. If next <= now: job is due
```

**First-run behavior:** When `last_run` is NULL (new job), defaults to `now` so the job
waits for its next scheduled occurrence instead of firing immediately on the first tick.

#### 2.2.9 History and Notifications

After each job execution:

```
1. update_cron_job_last_run(job.id, error_message)  // best-effort
2. update_cron_history(history.id, success, output, error)  // best-effort
3. Desktop notification:
   - Success: "{job.name} completed"
   - Failure: "{job.name} failed: {error}"
```

History updates are best-effort (non-critical tracking). Failures are logged but do not
prevent future job execution.

### 2.3 Heartbeat Scheduler

**File:** `crates/server/src/heartbeat.rs`

The heartbeat scheduler fires proactive AI prompts for entities (main agent, persona
agents, channels) that have heartbeat enabled. It coexists with AgentWorker
workflow-bound heartbeats: AgentWorker runs workflows, this scheduler runs prompt-based
chat dispatches.

#### 2.3.1 Lifecycle

```
spawn(state)
  |
  +-> 15s initial delay (let server boot)
  |
  +-> Seed last-fire times from DB
  |     +-> list_heartbeat_entities()
  |     +-> For each entity with last_heartbeat_at:
  |           +-> Convert epoch to Instant (accounting for elapsed time)
  |           +-> Insert into LastFired map
  |
  +-> loop {
        interval.tick() (60s)
        |
        +-> tick(state, last_fired)
              +-> Load global settings
              +-> Load global tool permissions
              +-> Read HEARTBEAT.md content
              +-> Collect heartbeat entities (explicit + main if global interval > 0)
              +-> For each entity:
                    +-> resolve entity config (inheritance)
                    +-> Check enabled + interval > 0
                    +-> Check elapsed time >= interval
                    +-> Check time window (HH:MM start/end)
                    +-> Check content not empty
                    +-> Check agent is active (if entity_type == "agent")
                    +-> Fire heartbeat via run_chat()
                    +-> Persist last-fire to DB
      }
```

#### 2.3.2 Entity Config Resolution

```
+-------------------+     +-------------------+     +-------------------+
| Global Settings   |     | EntityConfig Row  |     | ResolvedConfig    |
| heartbeat_interval|---->| heartbeat_enabled |---->| heartbeat_enabled |
| tool_permissions  |     | heartbeat_interval|     | heartbeat_interval|
|                   |     | heartbeat_content |     | heartbeat_content |
+-------------------+     | heartbeat_window  |     | heartbeat_window  |
                          +-------------------+     | permissions       |
                                                    +-------------------+
```

Entity config uses inheritance: per-entity overrides layer on top of global defaults.
The `entity_config::resolve()` function handles this layering.

#### 2.3.3 Time Window Support

```rust
fn in_time_window(start: &str, end: &str) -> bool {
    let now = chrono::Local::now().format("%H:%M");
    if start <= end {
        now >= start && now <= end        // Same-day window (e.g., 09:00 - 17:00)
    } else {
        now >= start || now <= end        // Midnight-wrapping (e.g., 22:00 - 06:00)
    }
}
```

Heartbeats can be constrained to specific hours (e.g., business hours only).

#### 2.3.4 Persistence Across Restarts

Last-fire times are stored both in memory (`LastFired` map) and in the DB
(`entity_config.last_heartbeat_at` as Unix epoch string). On startup, the scheduler
seeds the in-memory map from DB values, accounting for elapsed wall-clock time. This
prevents heartbeats from re-firing immediately after a server restart.

#### 2.3.5 ChatConfig Construction

```rust
ChatConfig {
    session_key: "heartbeat-{entity_type}-{entity_id}",
    prompt: resolved.heartbeat_content,
    channel: "heartbeat",
    origin: Origin::System,
    lane: lanes::HEARTBEAT,
    entity_config: Some(resolved),
    ...
}
```

Heartbeats use the `HEARTBEAT` lane for queue separation and concurrency control.

### 2.4 Memory Consolidation Sweep

**File:** `crates/agent/src/memory_consolidation.rs`

Background deduplication, merging, and pruning of memories. Spawned from
`lib.rs` after runner creation.

```
spawn_sweep(store, providers)
  |
  +-> 60s initial delay
  |
  +-> loop {
        interval.tick() (30 minutes)
        |
        +-> Gate 1: is_enabled(store)
        +-> Get distinct memory user_id scopes
        +-> For each scope:
              +-> Gate 2: count >= 20 (MIN_MEMORIES_FOR_CONSOLIDATION)
              +-> Run consolidation (dedup, merge, prune)
      }
```

| Constant | Value | Description |
|----------|-------|-------------|
| `SWEEP_INTERVAL_MINUTES` | 30 | Minutes between sweeps |
| `MIN_MEMORIES_FOR_CONSOLIDATION` | 20 | Minimum memories before consolidation runs |

### 2.5 Resource Monitor (Concurrency Controller)

**File:** `crates/agent/src/concurrency.rs`

Adaptive concurrency controller that adjusts agent parallelism based on system resources.

```
spawn_monitor(controller)
  |
  +-> loop {
        interval.tick() (30s)
        |
        +-> spawn_blocking:
        |     sys.refresh_memory()
        |     available_mb = sys.available_memory() / 1MB
        |     load = System::load_average().one
        |
        +-> controller.update(available_mb, load)
      }
```

Uses `sysinfo` crate via `spawn_blocking` (blocking syscalls). Feeds CPU load and
available RAM into the concurrency controller which dynamically adjusts how many
concurrent agent runs are allowed.

### 2.6 Progress Broadcaster

**File:** `crates/server/src/lib.rs` (lines 1742-1755)

Broadcasts active run snapshots to all connected WebSocket clients every 5 seconds:

```rust
tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    loop {
        interval.tick().await;
        let runs = registry.list_top_level().await;
        if !runs.is_empty() {
            hub.broadcast("agent_progress", json!({ "runs": runs }));
        }
    }
});
```

Only emits when there are active runs (avoids noise). The frontend uses these events
to display real-time progress indicators for agent runs.

### 2.7 Tool Supervisor

**File:** `crates/server/src/lib.rs` (lines 931-957)

Health check loop for installed napp tool processes:

```
loop {
    interval.tick() (15s)
    |
    +-> list_processes()
    +-> For each tool:
          if not running && should_restart:
              record_restart()
              broadcast("tool_error", { toolId, error: "process died" })
}
```

Uses `napp::supervisor::Supervisor` for restart decision logic (backoff, max retries).

### 2.8 Background Update Checker

**File:** `crates/server/src/lib.rs` (lines 1632-1725)

Only runs in release builds (`!cfg!(debug_assertions)`):

```
updater::BackgroundChecker::new(
    VERSION,
    Duration::from_secs(3600),   // 1-hour interval
    on_update_available_callback
)
```

On update detection:
1. Broadcasts `update_available` WS event
2. If `can_auto_update && auto_update_enabled`:
   - Downloads update binary (with progress WS events)
   - Verifies checksum
   - Stages binary in `update_pending`
   - Broadcasts `update_ready` WS event

### 2.9 NeboLoop Reconnect Watcher

**File:** `crates/server/src/lib.rs` (lines 1567-1630)

Dual-trigger reconnection with exponential backoff and system sleep detection:

```
Initial delay: 60s

loop {
    select! {
        // Branch 1: periodic backoff poll
        sleep(backoff_secs)
        // Branch 2: instant disconnect notification
        comm_manager.wait_disconnect()
    }

    // Detect system sleep via wall-clock drift
    if elapsed_wall >> expected:
        shutdown stale connection
        force reconnect

    if connected:
        reset backoff to 30s
        continue

    activate_neboloop()
    |
    +-- OK: reset backoff to 30s, persist rotated JWT
    +-- Err: double backoff (max 600s = 10min)
}
```

Backoff progression: 30s -> 60s -> 120s -> 240s -> 480s -> 600s (capped).

### 2.10 Scheduler Timing Summary

```
+----------------------------------------------+
|           Background Task Schedule            |
+----------------------------------------------+
| Task                | Interval  | Init Delay  |
|---------------------|-----------|-------------|
| Cron scheduler      | 60s       | 10s         |
| Heartbeat scheduler | 60s       | 15s         |
| Memory consolidation| 30 min    | 60s         |
| Resource monitor    | 30s       | immediate   |
| Progress broadcaster| 5s        | immediate   |
| Tool supervisor     | 15s       | immediate   |
| Update checker      | 1 hour    | immediate   |
| NeboLoop reconnect  | 30s-600s  | 60s         |
| Snapshot cleanup    | 60s*      | 10s*        |
+----------------------------------------------+
* Piggybacks on cron scheduler tick
```

### 2.11 RunRegistry (Cross-System Coordination)

**File:** `crates/server/src/run_registry.rs`

The RunRegistry is the single source of truth for all active agent runs across the
system. Every entry point (WebSocket chat, REST API, cron, heartbeat, comm) registers
its run here.

#### 2.11.1 Data Model

```
RunRegistry
  |
  +-> inner: Arc<RunRegistryInner>
        |
        +-> runs: RwLock<HashMap<String, RunEntry>>
              |
              +-> RunEntry {
                    run_id: UUID
                    session_key: "agent:test:main"
                    entity_id: "test-agent"
                    entity_name: "Test Agent"
                    origin: "ws" | "cron" | "heartbeat" | "comm"
                    channel: "main" | "cron" | "heartbeat"
                    cancel_token: CancellationToken
                    started_at: Instant
                    last_activity: AtomicU64 (epoch secs)
                    iteration_count: AtomicU32
                    tool_call_count: AtomicU32
                    current_tool: Mutex<String>
                    parent_run_id: Option<String>
                  }
```

#### 2.11.2 RunHandle (RAII Pattern)

```rust
pub struct RunHandle {
    registry: Arc<RunRegistryInner>,
    pub run_id: String,
    pub last_activity: Arc<AtomicU64>,
    pub iteration_count: Arc<AtomicU32>,
    pub tool_call_count: Arc<AtomicU32>,
    pub current_tool: Arc<std::sync::Mutex<String>>,
    pub cancel_token: CancellationToken,
}
```

The `RunHandle` is returned from `register()` and auto-unregisters the run when dropped
(via `impl Drop`). This makes it panic-safe -- even if a run panics or errors out, the
registry entry is cleaned up. The `Drop` implementation first tries a synchronous
`try_write()` lock, falling back to spawning a Tokio task for async cleanup.

#### 2.11.3 Query API

| Method | Description |
|--------|-------------|
| `list_all()` | All active runs (full visibility) |
| `list_top_level()` | Only parent runs (no sub-agents) |
| `list_children(parent_id)` | Direct children of a run |
| `get(run_id)` | Single run lookup |
| `find_by_session(key)` | Find by session key |
| `find_by_entity(id)` | All runs for an entity + descendants |
| `is_session_active(key)` | Boolean check |
| `active_count()` | Total active runs |
| `cleanup_stale(max_idle)` | Cancel + remove idle runs |

#### 2.11.4 Cancellation

| Method | Scope |
|--------|-------|
| `cancel(run_id)` | Single run |
| `cancel_by_session(key)` | By session key |
| `cancel_by_entity(id)` | All runs for an entity |
| `cancel_all()` | Emergency kill -- everything |

Cancellation cascades to children via `CancellationToken` parent-child relationships.
The token tree is set up by the caller (e.g., child agents use `parent_token.child_token()`).

#### 2.11.5 Authorization (RunQuerier Trait)

The RunRegistry implements `tools::run_querier::RunQuerier` for cross-crate access:

- **Main agent:** Can see and cancel all runs
- **Persona agents:** Can only see their own runs + descendants; can only cancel runs
  in their own tree (verified by walking the parent chain)

### 2.12 Graceful Shutdown

**File:** `crates/server/src/lib.rs` (lines 1848-1870)

```
SIGTERM or Ctrl+C received
  |
  +-> Drain in-flight memory extractions (memory_flush::drain_extractions)
  +-> Stop all app sidecars (AppLifecycle::shutdown for each)
  +-> Disconnect comm plugins (comm_manager::shutdown)
  +-> 100ms pause (let WebSocket Close frames send)
  +-> Server exits
```

The shutdown signal handler supports both Unix signals (SIGTERM) and cross-platform
Ctrl+C. Memory extractions are drained first to prevent data loss.

### 2.13 Concurrency and Duplicate Prevention

The system uses several mechanisms to prevent duplicate or runaway execution:

1. **RunRegistry:** Tracks all active runs; `is_session_active()` can be checked before
   starting a new run for the same session.

2. **CancellationToken:** Every run carries a token; parent-child relationships cascade
   cancellation through the entire sub-agent tree.

3. **LaneManager:** Per-lane task queuing with configurable concurrency limits prevents
   too many runs from executing simultaneously in the same lane.

4. **ConcurrencyController:** System-resource-aware throttling that dynamically adjusts
   based on available RAM and CPU load.

5. **Cron last_run tracking:** Each cron job records its `last_run` timestamp. The
   scheduler compares this against the next scheduled occurrence to determine if a job
   is due, preventing double-execution if the 60s poll interval overlaps.

6. **Heartbeat last_fired map:** In-memory + DB persistence prevents re-firing after
   restart. The `Duration` check (`elapsed >= interval`) prevents duplicate fires
   within the same scheduler tick.

7. **WorkflowManager active_runs:** Tracks active workflow cancellation tokens, keyed
   by `run_id`, with a secondary `agent_runs` map for per-agent cancellation.

---

## Appendix: Configuration Reference

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `NEBO_ALLOW_REMOTE` | unset | Set to `"true"` to allow non-localhost binding |
| `NEBO_MCP_API_KEY` | unset | API key for MCP endpoint (optional) |

### Server Ports

| Port | Use |
|------|-----|
| 27895 | Production server (Axum) |
| 5173 | Vite dev server (proxies to 27895) |
| 4173 | Vite preview server |

### Key Constants

| Constant | Value | Location |
|----------|-------|----------|
| Auth rate limit | 10 req/60s | `routes/mod.rs` |
| Cron poll interval | 60s | `scheduler.rs` |
| Cron initial delay | 10s | `scheduler.rs` |
| Heartbeat poll interval | 60s | `heartbeat.rs` |
| Heartbeat initial delay | 15s | `heartbeat.rs` |
| Memory consolidation interval | 30 min | `memory_consolidation.rs` |
| Memory consolidation delay | 60s | `memory_consolidation.rs` |
| Min memories for consolidation | 20 | `memory_consolidation.rs` |
| Resource monitor interval | 30s | `concurrency.rs` |
| Progress broadcast interval | 5s | `lib.rs` |
| Tool supervisor interval | 15s | `lib.rs` |
| Update check interval | 1 hour | `lib.rs` |
| NeboLoop reconnect backoff | 30s-600s | `lib.rs` |
| Completed task TTL | 7 days | `scheduler.rs` (tick cleanup) |
