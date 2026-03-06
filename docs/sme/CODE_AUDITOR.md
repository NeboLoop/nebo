# Code Auditor — Internal Reference (Rust)

This document makes any Claude Code session an immediate expert on Nebo-rs code quality rules. Read it before writing or reviewing any code in this repository.

**No external documentation exists.** Everything below is derived from source code and project conventions.

---

## The Rules

| # | Rule | Enforcement Point |
|---|------|--------------------|
| 1 | **Reuse and edit existing code** | Before writing anything new, search for existing handlers, store methods, types, and utilities |
| 2 | **Create when needed** | Only after confirming no existing code solves the problem |
| 3 | **No dead code** | Delete unused handlers, types, queries, imports — never comment out or leave orphaned |
| 4 | **Build before pushing** | `cargo build` must pass clean (no warnings, no errors) before any push |
| 5 | **Use generated TS API code** | Frontend must import from `$lib/api/nebo` — never manual `fetch()` calls |
| 6 | **Rust patterns** | Error handling, naming, traits, extractors — follow established idioms |
| 7 | **API design standards** | Response format, pagination, path conventions, type conventions |
| 8 | **Architecture** | No competing pathways, boundary awareness, dependency direction, handler-owns-logic |
| 9 | **Concurrency safety** | No blocking under async locks, no lock ordering violations, Arc discipline, CancellationToken for runs |

---

## Rule 1: Reuse and Edit Existing Code

### Philosophy

When solving a problem, **scan existing code first**. The codebase follows one pattern per concern — duplicating it is a violation. Every handler, store method, type, and utility was written to be reused.

### Where to Look

#### Handlers (`crates/server/src/handlers/`)

Every handler module groups a domain. Before creating a new handler, check if one already exists for that entity.

```
crates/server/src/handlers/
├── mod.rs           ← HandlerResult type, to_error_response helper, ErrorResponse struct
├── agent.rs         ← Agent settings, status, advisors, memory, lanes
├── auth.rs          ← Login, register, token refresh
├── chat.rs          ← Chat CRUD, messaging, search, history
├── extensions.rs    ← Skills CRUD, toggle
├── integration.rs   ← MCP integrations CRUD, test
├── notification.rs  ← Notification CRUD, read/unread
├── plugins.rs       ← Plugin settings, store apps/skills
├── provider.rs      ← Auth profiles (API keys), models, task routing
├── setup.rs         ← First-run setup wizard
├── tasks.rs         ← Scheduled tasks CRUD, toggle, run, history
├── user.rs          ← User profile, preferences
└── ws.rs            ← WebSocket handler, ClientHub, message dispatch
```

#### The Handler Pattern

Every handler follows exactly this structure — copy it, don't invent a new one:

```rust
// crates/server/src/handlers/chat.rs
pub async fn list_chats(
    State(state): State<AppState>,
    Query(q): Query<ListChatsQuery>,
) -> HandlerResult<serde_json::Value> {
    let chats = state
        .store
        .list_chats(q.limit.unwrap_or(50), q.offset.unwrap_or(0))
        .map_err(to_error_response)?;

    Ok(Json(serde_json::json!({
        "chats": chats,
        "total": chats.len()
    })))
}
```

**Key invariants:**
- Extract `State(state): State<AppState>` — shared application state
- Extract request data via `Path(id)`, `Query(q)`, or `Json(body)` — Axum extractors
- Return `HandlerResult<T>` which is `Result<Json<T>, (StatusCode, Json<ErrorResponse>)>`
- Convert errors with `.map_err(to_error_response)?`
- DB access through `state.store.*` — never create direct DB connections
- Auth claims via `axum::Extension(claims): axum::Extension<AuthClaims>`

#### Handler Result Type (`crates/server/src/handlers/mod.rs`)

Never write custom response logic. These exist:

| Type / Function | Purpose |
|-----------------|---------|
| `HandlerResult<T>` | `Result<Json<T>, (StatusCode, Json<ErrorResponse>)>` — the standard return |
| `to_error_response(e)` | Converts `NeboError` → `(StatusCode, Json<ErrorResponse>)` |
| `ErrorResponse` | `{ error: String }` — standard error body |

```rust
pub type HandlerResult<T> = Result<Json<T>, (StatusCode, Json<ErrorResponse>)>;

pub fn to_error_response(e: NeboError) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(ErrorResponse { error: e.to_string() }),
    )
}
```

#### Axum Extractors

| Extractor | Purpose | Example |
|-----------|---------|---------|
| `State(state): State<AppState>` | Shared application state | Every handler |
| `Path(id): Path<String>` | URL path parameter | `GET /chats/{id}` |
| `Query(q): Query<ListQuery>` | Query string parameters | `?limit=50&offset=0` |
| `Json(body): Json<CreateRequest>` | JSON request body | `POST /chats` |
| `Extension(claims): Extension<AuthClaims>` | JWT auth claims (from middleware) | Protected routes |
| `WebSocketUpgrade` | WebSocket upgrade | `GET /ws` |

#### Error Type (`crates/types/src/error.rs`)

All errors use one enum with HTTP status code mapping:

```rust
#[derive(Debug, Error)]
pub enum NeboError {
    #[error("user not found")]
    UserNotFound,                        // 404
    #[error("invalid credentials")]
    InvalidCredentials,                  // 401
    #[error("unauthorized")]
    Unauthorized,                        // 401
    #[error("not found")]
    NotFound,                            // 404
    #[error("database error: {0}")]
    Database(String),                    // 500
    #[error("rate limit exceeded")]
    RateLimit,                           // 429
    #[error("validation error: {0}")]
    Validation(String),                  // 500
    #[error("{0}")]
    Internal(String),                    // 500
}
```

#### Store (`crates/db/src/store.rs`)

All DB methods live on the `Store` struct. Before writing a new query, check if one exists:

```bash
# Search existing store methods
grep -r "pub fn.*get_widget\|pub fn.*list_widget" crates/db/src/
```

If no method exists, add it to the appropriate query module in `crates/db/src/` and expose it via `Store`.

#### AppState (`crates/server/src/state.rs`)

Central dependency container. Handlers receive it via `State` extractor — never instantiate dependencies yourself.

Key fields: `config`, `store`, `auth`, `hub`, `runner`, `tools`, `bridge`, `models_config`, `approval_channels`, `ask_channels`

```rust
#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub store: Arc<Store>,
    pub auth: Arc<AuthService>,
    pub hub: Arc<ClientHub>,
    pub runner: Arc<Runner>,
    pub tools: Arc<Registry>,
    pub bridge: Arc<mcp::Bridge>,
    pub models_config: Arc<config::ModelsConfig>,
    pub cli_statuses: Arc<config::AllCliStatuses>,
    pub approval_channels: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
    pub ask_channels: Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>,
}
```

---

## Rule 2: Create When Needed

Only create new code after confirming nothing existing solves the problem. When you do create, follow the established patterns exactly.

### Adding a New Endpoint (Step-by-Step)

**Step 1: Define query/body types** (in the handler file or a shared types module):

```rust
#[derive(Deserialize)]
pub struct GetWidgetQuery {
    pub format: Option<String>,
}
```

**Step 2: Create handler** in `crates/server/src/handlers/{domain}.rs`:

```rust
pub async fn get_widget(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let widget = state
        .store
        .get_widget(&id)
        .map_err(to_error_response)?;

    Ok(Json(serde_json::json!({
        "widget": widget
    })))
}
```

**Step 3: Register route** in `crates/server/src/lib.rs` inside `api_routes()`:

Routes go in one of three places depending on auth requirements:

| Section | Auth | Use |
|---------|------|-----|
| `public` routes | None | Most routes (single-user app) |
| `protected` routes | JWT required | Multi-user sensitive operations |
| `auth_routes` | Rate-limited, no JWT | Login/register |

```rust
// In api_routes(), add to public Router:
.route("/widgets/{id}", axum::routing::get(handlers::widget::get_widget))
```

**Step 4: Build and verify** — `cargo build` must pass clean.

**Step 5: Update TS API client** if frontend needs access to the new endpoint.

### Adding a New DB Table

1. Create migration: `crates/db/migrations/XXXX_description.sql` (4-digit prefix, goose format with `-- +goose Up` / `-- +goose Down`)
2. Add query methods to appropriate module in `crates/db/src/`
3. Expose via `Store` impl
4. Use via `state.store.*` in handlers

### Adding a New Crate

Only when the functionality genuinely doesn't belong in any existing crate:

1. Create `crates/newcrate/Cargo.toml` with `workspace = true` dependencies
2. Add to `[workspace.members]` in root `Cargo.toml`
3. Add `newcrate = { path = "crates/newcrate" }` to `[workspace.dependencies]`
4. Follow the existing pattern: `src/lib.rs` as entry, re-export public API

---

## Rule 3: No Dead Code

Dead code is any code that compiles but is never executed. It creates confusion, increases maintenance burden, and misleads future developers.

### What Counts as Dead Code

| Pattern | Example | Required Action |
|---------|---------|-----------------|
| Unreferenced handler | Handler function defined but never routed in `lib.rs` | Delete the handler |
| Orphaned type | Struct with zero usages outside its definition | Delete the struct |
| Unused store method | Store method never called from any handler or service | Delete the method |
| Unreferenced import | `use some::unused::module` | Remove (`rustc` warns, but check `#[allow(unused)]`) |
| Unused helper | Function with zero callers | Delete |
| Commented-out code | `// old_function()` blocks left behind | Delete entirely |
| Dead feature module | Entire module replaced by new implementation | Delete after confirming no references |
| `#[allow(dead_code)]` | Suppressed warning without justification | Remove suppression and delete the dead code |

### How to Verify

```bash
# Check if a handler is routed
grep -r "get_widget" crates/server/src/lib.rs

# Check if a type is used anywhere
grep -r "GetWidgetResponse" --include="*.rs" .

# Check if a store method is called
grep -r "store.get_widget\|store\.get_widget" --include="*.rs" .

# Find all dead_code suppressions
grep -rn "allow(dead_code)" --include="*.rs" .
```

If a reference only appears in its own definition, it's dead. Delete it.

### Cleanup Checklist (When Removing a Feature)

1. Remove route registration from `crates/server/src/lib.rs`
2. Delete handler function from `crates/server/src/handlers/{domain}.rs`
3. Remove query/body structs if only used by that handler
4. Delete store methods from `crates/db/src/` if applicable
5. Delete migration SQL if the table is being removed (add a down migration)
6. Search codebase for remaining references: `grep -r "FeatureName" --include="*.rs" .`
7. Remove empty handler modules from `mod.rs`
8. `cargo build` — verify no compile errors or warnings

---

## Rule 4: Build Before Pushing

### The Build Pipeline

```
cargo build                    ← Compile all workspace crates
        ↓
cargo test                     ← Run all tests (72+ across crates)
        ↓
cd ../../app && pnpm check     ← TypeScript type checking (if frontend changed)
```

### When to Build

- After any code change before pushing
- After adding/removing routes
- After changing types or store methods
- After modifying `Cargo.toml` dependencies

### Zero Warnings Policy

All warnings must be resolved. Common offenders:

| Warning | Fix |
|---------|-----|
| `unused import` | Remove the import |
| `unused variable` | Prefix with `_` only if intentionally unused, otherwise delete |
| `unnecessary mut` | Remove `mut` keyword |
| `dead_code` | Delete the code (see Rule 3) |
| `unused must_use` | Handle the `Result` or explicitly discard with `let _ =` with a comment |

**Never use `#[allow(warnings)]` or `#[allow(dead_code)]`** without an explicit comment explaining why.

---

## Rule 5: Use Generated TypeScript API Client

### The Wrong Way (Never Do This)

```typescript
// WRONG: Manual fetch calls
const resp = await fetch(`/api/v1/widgets/${id}`, {
    headers: { Authorization: `Bearer ${token}` }
})
const data = await resp.json()
```

Problems: no type safety, duplicates paths, breaks silently if routes change, no autocomplete.

### The Right Way (Always)

```typescript
// CORRECT: Import from generated client
import { getWidget } from '$lib/api/nebo'

const widget = await getWidget(id)  // Fully typed, autocomplete works
```

Frontend must always use the generated TypeScript API client. When backend routes change, the TS client must be regenerated to stay in sync.

---

## Section 6: Rust Patterns

These are the established Rust idioms enforced in Nebo-rs. Each subsection shows the **standard pattern** with a code example, then a **Don't** counterexample.

### 6.1 Error Handling

Use `?` with `.map_err()` to propagate errors. Never silently discard errors.

```rust
// Standard pattern — convert error and propagate
let chat = state
    .store
    .get_chat(&id)
    .map_err(to_error_response)?;
```

Use pattern matching for specific error variants — never string comparison:

```rust
// CORRECT — pattern match on error variant
match state.store.get_user_by_id(&id) {
    Ok(Some(user)) => Ok(Json(serde_json::json!({"user": user}))),
    Ok(None) => Err(to_error_response(NeboError::UserNotFound)),
    Err(e) => Err(to_error_response(e)),
}

// WRONG — string matching on error messages
if err.to_string().contains("not found") { ... }
```

Use `thiserror` `#[from]` for automatic conversions — never manual `impl From`:

```rust
// CORRECT — thiserror derives From
#[derive(Debug, Error)]
pub enum NeboError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

// WRONG — manual From impl when thiserror handles it
impl From<std::io::Error> for NeboError {
    fn from(e: std::io::Error) -> Self { Self::Io(e) }
}
```

Log with context AND return — never silently discard:

```rust
// CORRECT — log and return
if let Err(e) = state.store.cleanup_old_sessions() {
    tracing::warn!("session cleanup failed: {}", e);
}

// WRONG — silent discard without comment
let _ = state.store.cleanup_old_sessions();
```

### 6.2 Naming

**Handler functions:** `snake_case` verbs matching the route purpose:

```
list_chats          ✅
get_chat            ✅
create_chat         ✅
update_chat         ✅
delete_chat         ✅
ListChatsHandler    ❌  (Go style)
handleListChats     ❌
```

**Handler files:** one file per domain, named by domain:

```
chat.rs             ✅
agent.rs            ✅
list_chats.rs       ❌  (one handler per file is Go style)
```

**Standard variable names** — these are non-negotiable:

| Variable | Meaning | Wrong Alternatives |
|----------|---------|-------------------|
| `state` | AppState from `State(state)` | `s`, `app`, `app_state`, `ctx` |
| `id` | Path parameter ID | `widget_id` (unless ambiguous) |
| `body` | JSON body from `Json(body)` | `payload`, `data`, `input` |
| `q` | Query params from `Query(q)` | `query`, `params`, `qs` |
| `claims` | Auth claims from `Extension(claims)` | `auth`, `user`, `token` |
| `conn` | DB connection from pool | `db`, `database`, `c` |

**Crate names:** single lowercase word by domain:

```
types/              ✅
config/             ✅
user_auth/          ❌
userAuth/           ❌
```

**Module names:** `snake_case`, descriptive:

```
pub mod policy;     ✅
pub mod tool_policy; ❌  (too specific when inside tools crate)
```

### 6.3 Traits

Small and focused. Defined where consumed (consumer-side), not where implemented.

```rust
// CORRECT — trait defined in agent crate where it's consumed
// crates/agent/src/provider.rs
#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    async fn chat(&self, messages: &[Message], tools: &[Tool]) -> Result<Response, NeboError>;
}

// Implemented in ai crate:
// crates/ai/src/anthropic.rs
impl Provider for AnthropicProvider { ... }
```

Use trait objects (`dyn Trait`) with `Arc` for runtime polymorphism:

```rust
// CORRECT
pub struct Runner {
    providers: Arc<RwLock<Vec<Box<dyn Provider>>>>,
}

// WRONG — generic parameter explosion
pub struct Runner<P: Provider> { ... }
```

Use `Arc<dyn Trait>` when the trait object must be shared across tasks — `Box<dyn Trait>` when single-owner:

```rust
// Shared across async tasks
pub tools: Arc<Registry>,

// Single owner, stored in a Vec
providers: Vec<Box<dyn Provider>>,
```

### 6.4 Dependency Injection

`AppState` is the sole DI container — never create dependencies in handlers:

```rust
// WRONG — creating a DB connection in a handler
let conn = rusqlite::Connection::open("nebo.db")?;

// CORRECT — always use state
let result = state.store.get_widget(&id).map_err(to_error_response)?;
```

**Late-bound services:** Initialize at startup in `run()`, store in `AppState`. Use `Arc` for shared ownership:

```rust
// crates/server/src/lib.rs — run() function
let store = Arc::new(Store::new(&db_path)?);
let auth_service = Arc::new(AuthService::new(store.clone(), cfg.clone()));
let hub = Arc::new(ClientHub::new());
// ... build AppState with all services
```

Never use global state (`lazy_static`, `once_cell::sync::Lazy` for mutable state) — everything flows through `AppState`:

```rust
// WRONG — global mutable state
static STORE: Lazy<Mutex<Store>> = Lazy::new(|| ...);

// CORRECT — passed through AppState
pub store: Arc<Store>,
```

### 6.5 Serde Conventions

All serializable types use `#[derive(Serialize, Deserialize)]` with explicit field naming:

```rust
#[derive(Serialize, Deserialize)]
pub struct Chat {
    pub id: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "isActive")]
    pub is_active: bool,
}
```

**JSON field casing:** always `camelCase` for HTTP response types (use `#[serde(rename)]` or `#[serde(rename_all = "camelCase")]`):

```rust
// CORRECT
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatResponse {
    pub chat_id: String,      // → "chatId"
    pub created_at: String,   // → "createdAt"
}

// WRONG — snake_case in JSON output
pub struct ChatResponse {
    pub chat_id: String,      // → "chat_id" ❌
}
```

**Acceptable snake_case exceptions** (do NOT convert these):
- OAuth RFC fields (`access_token`, `refresh_token`, `token_type`)
- Anthropic API fields (`tool_call_id`, `is_error`, `stop_reason`)
- DB model fields used only internally (never serialized to frontend)

**Optional fields:** use `Option<T>` + `#[serde(skip_serializing_if = "Option::is_none")]`:

```rust
#[derive(Serialize)]
pub struct Widget {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}
```

### 6.6 Concurrency

Full concurrency section in Rule 9 below. This subsection covers the **rules for writing new concurrent code**.

#### Choosing the Right Primitive

```
Is it shared across async tasks (tokio::spawn boundaries)?
  YES → Arc<T> wrapper required
  NO  ↓

Is it a single atomic value (bool, u32, usize)?
  YES → AtomicBool / AtomicU32 / AtomicUsize
  NO  ↓

Is it read-heavy with rare writes?
  YES → tokio::sync::RwLock (async) or std::sync::RwLock (sync)
  NO  ↓

Does it need to be held across .await points?
  YES → tokio::sync::Mutex (async)
  NO  → std::sync::Mutex (sync, cheaper)
```

**Never mix `std::sync::Mutex` with async code that holds the lock across `.await`** — this blocks the tokio runtime thread:

```rust
// WRONG — std::sync::Mutex held across .await
let guard = std_mutex.lock().unwrap();
some_async_fn().await;  // BLOCKS the runtime thread
drop(guard);

// CORRECT — use tokio::sync::Mutex for async
let guard = tokio_mutex.lock().await;
some_async_fn().await;
drop(guard);

// ALSO CORRECT — std::sync::Mutex is fine if no .await while held
let value = {
    let guard = std_mutex.lock().unwrap();
    guard.clone()
};  // lock dropped before any .await
```

---

## Section 7: API Design Standards

HTTP API conventions derived from all existing endpoints.

### 7.1 Response Format

**Success:** flat response (no `{data: ...}` envelope):

```rust
// CORRECT — flat
Ok(Json(serde_json::json!({
    "user": user_data
})))

// WRONG — wrapped
Ok(Json(serde_json::json!({
    "data": { "user": user_data }
})))
```

**Collections:** `{pluralKey: [...], total: N}`:

```rust
Ok(Json(serde_json::json!({
    "chats": chats,
    "total": total
})))
```

**Single resource:** `{singularKey: {...}}`:

```rust
Ok(Json(serde_json::json!({
    "chat": chat
})))
```

**Errors:** `{error: String}` (from `ErrorResponse`).

**JSON field casing:** always `camelCase` (see Section 6.5).

### 7.2 Pagination

Offset-based: `limit` + `offset` as query params:

```rust
#[derive(Deserialize)]
pub struct ListQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}
```

Hard cap in handler — clamp values:

```rust
let limit = q.limit.unwrap_or(50).min(100);
let offset = q.offset.unwrap_or(0).max(0);
```

Always return `total` count in response.

### 7.3 Path Conventions

| Pattern | Example | When |
|---------|---------|------|
| Plural resources | `/chats`, `/memories`, `/providers` | Always |
| Standard CRUD | `GET /`, `POST /`, `GET /{id}`, `PUT /{id}`, `DELETE /{id}` | Entity operations |
| Actions | `POST /resource/{id}/verb` | State changes, triggers |
| Sub-resources | `/parent/{id}/children` | Nested entities |

Real examples from `lib.rs`:

```rust
.route("/skills/{name}/toggle", axum::routing::post(handlers::extensions::toggle_skill))
.route("/tasks/{name}/toggle", axum::routing::post(handlers::tasks::toggle_task))
.route("/tasks/{name}/run", axum::routing::post(handlers::tasks::run_task))
```

No trailing slashes.

### 7.4 HTTP Methods

| Method | Use | Response |
|--------|-----|----------|
| GET | Reads (never mutates) | Resource or collection |
| POST | Creates + actions (toggles, runs, tests) | Created resource or result |
| PUT | Full updates (no PATCH in this codebase) | Updated resource |
| DELETE | Deletes | Message response |

DELETE always returns a message:

```rust
Ok(Json(serde_json::json!({
    "message": "Chat deleted"
})))
```

### 7.5 Route Organization in `lib.rs`

Routes are organized in `api_routes()`:

```rust
fn api_routes(jwt_secret: JwtSecret) -> Router<AppState> {
    // 1. Auth routes — rate-limited, no JWT
    let auth_routes = Router::new()
        .route("/auth/login", post(handlers::auth::login))
        .route("/auth/register", post(handlers::auth::register))
        .layer(/* rate limiter */);

    // 2. Public routes — no auth required
    let public = Router::new()
        .route("/chats", get(handlers::chat::list_chats))
        .route("/chats", post(handlers::chat::create_chat))
        // ... grouped by domain

    // 3. Protected routes — JWT required
    let protected = Router::new()
        .route("/user/me", get(handlers::user::get_current_user))
        .layer(axum::middleware::from_fn(middleware::jwt_auth));

    // 4. Merge all
    Router::new()
        .merge(auth_routes)
        .merge(public)
        .merge(protected)
}
```

**WebSocket route** is kept at the top level, OUTSIDE `CompressionLayer` (compression corrupts WebSocket frames):

```rust
let app = Router::new()
    .route("/ws", get(handlers::ws::client_ws_handler))  // ← outside compression
    .merge(http_routes)                                    // ← http_routes has compression
    .with_state(state);
```

---

## Section 8: Architecture

Layering, dependency direction, code boundaries, and structural patterns. **This section prevents the most damaging vibe-coding mistakes.**

### 8.1 No Competing Pathways (CRITICAL)

The #1 architecture rule: if functionality exists in one place, it must NOT be duplicated in another. Every capability has ONE canonical implementation.

**What competing pathways look like:**

| Violation | Example |
|-----------|---------|
| Two HTTP clients calling the same external API | Two modules wrapping the same REST API |
| Two modules providing the same abstraction | A new `crates/utils/` when `crates/types/` already handles it |
| Internal wrappers that duplicate a dependency's surface | Writing `fn get_user(conn: &Connection, id: &str)` when `store.get_user_by_id()` exists |
| "Convenience" functions that reimplement imported functionality | Reimplementing JSON parsing when Axum extractors handle it |

**The auditor must flag this pattern wherever it appears.**

### 8.2 Know Your Boundaries

Before editing any file, understand what crate it belongs to and what the edit implies:

| Boundary | What It Means | Example |
|----------|---------------|---------|
| **This workspace** (`crates/*/`) | Direct edit, `cargo build` | Editing a handler |
| **External dependency** (`Cargo.toml`) | Cannot edit — wrap or fork | Using `reqwest` |
| **Generated code** (`app/src/lib/api/`) | Never edit directly — regenerate | TypeScript API client |
| **Migration files** (applied) | Never modify applied migrations — create new one | Altering a column |

### 8.3 Handlers Own Business Logic

Handlers directly call `state.store.*` — no separate logic/repository/DAO layer:

```rust
pub async fn list_chats(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> HandlerResult<serde_json::Value> {
    // 1. Extract params (Axum does this via extractors)
    let limit = q.limit.unwrap_or(50).min(100);

    // 2. DB call (directly — no intermediary)
    let chats = state.store.list_chats(limit, 0).map_err(to_error_response)?;

    // 3. Format response
    Ok(Json(serde_json::json!({ "chats": chats, "total": chats.len() })))
}
```

This is the pattern. Do not create `crates/logic/` or intermediary service layers between handlers and store.

If logic is shared across handlers, extract to a utility function in the same handler module or in the relevant crate — not a separate layer.

### 8.4 Dependency Direction

```
server (handlers, routes, middleware)
  ├─ types (errors, constants)
  ├─ db (Store, models, migrations)
  ├─ auth (AuthService, JWT)
  ├─ config (Config, ModelsConfig)
  ├─ agent (Runner, SessionManager)
  ├─ tools (Registry, Policy)
  ├─ ai (Provider trait, implementations)
  ├─ mcp (Bridge, Client)
  ├─ updater (BackgroundChecker)
  └─ notify
```

No circular imports — Rust's module system enforces this at compile time, but **crate-level cycles** are still possible if you're not careful. Before adding a dependency between crates, verify it doesn't create a cycle:

```bash
# Check if crate A already depends on crate B
grep "crate-b" crates/crate-a/Cargo.toml
```

Never introduce a new dependency direction that doesn't already exist. If crate A has never depended on crate B, question whether it should start now.

### 8.5 Database Access

- Always through `state.store.*` (typed methods on `Store`)
- New queries: add method to `Store` in `crates/db/src/`
- No raw SQL strings in handler code
- No ORM — hand-written SQL with `rusqlite` parameter binding

```rust
// CORRECT
let chats = state.store.list_chats(limit, offset).map_err(to_error_response)?;

// WRONG — raw SQL in handler
let conn = state.store.conn()?;
conn.query_row("SELECT * FROM chats WHERE ...", [], |row| ...)?;
```

**Store method pattern:**

```rust
// crates/db/src/store.rs or crates/db/src/queries/{domain}.rs
impl Store {
    pub fn get_widget(&self, id: &str) -> Result<Widget, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, name, created_at FROM widgets WHERE id = ?",
            [id],
            |row| Ok(Widget {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
            }),
        )
        .map_err(|e| NeboError::Database(format!("get_widget: {e}")))
    }
}
```

### 8.6 Middleware

Applied via Axum's `layer()` system:

```rust
// JWT auth on specific routes
let protected = Router::new()
    .route("/user/me", get(handlers::user::get_current_user))
    .layer(axum::middleware::from_fn(middleware::jwt_auth));

// Rate limiting with state
let auth_routes = Router::new()
    .route("/auth/login", post(handlers::auth::login))
    .layer(Extension(auth_limiter))
    .layer(axum::middleware::from_fn(middleware::rate_limit));

// Global middleware — applied to all routes
let app = Router::new()
    .merge(routes)
    .layer(middleware::security_headers)
    .layer(cors_layer());
```

Middleware ordering in Axum: **last applied = first executed**. Security headers and CORS go outermost (applied last).

### 8.7 Workspace Crate Organization

| Crate | Purpose | Depends On |
|-------|---------|------------|
| `types` | Error enum, constants, shared types | (none — leaf crate) |
| `config` | Config structs, YAML loading, CLI detection | `types` |
| `db` | Store, migrations, connection pool | `types` |
| `auth` | AuthService, JWT validation/generation | `types`, `db`, `config` |
| `ai` | Provider trait + implementations (Anthropic, OpenAI, Ollama) | `types` |
| `tools` | Tool registry, policy, shell/file/web tools | `types`, `db`, `ai` |
| `agent` | Runner, session, memory, compaction | `types`, `db`, `ai`, `tools` |
| `mcp` | Bridge, MCP client, encryption | `types`, `tools` |
| `browser` | Chrome management, CDP, automation | `types` |
| `voice` | Speech synthesis/recognition | `types` |
| `comm` | Communications, NeboLoop integration | `types` |
| `notify` | Notification system | `types` |
| `updater` | Background version checker | `types`, `config` |
| `cli` | CLI tool detection | `types`, `config` |
| `apps` | Third-party app integration | `types` |
| `server` | Axum server, handlers, routes, state | All of the above |

**Rule:** `types` is the leaf crate — it never depends on other workspace crates. All other crates depend on `types` for `NeboError`.

---

## Section 9: Concurrency Safety

### 9.1 Arc Discipline

Every value shared across `tokio::spawn` boundaries must be wrapped in `Arc`:

```rust
// CORRECT — Arc for cross-task sharing
let store = Arc::new(Store::new(&db_path)?);
let hub = Arc::new(ClientHub::new());

// WRONG — moving owned values into multiple tasks
let store = Store::new(&db_path)?;
tokio::spawn(async move { store.do_thing().await }); // store moved
tokio::spawn(async move { store.do_other() }); // compile error: moved
```

**Clone `Arc` before moving into spawned tasks:**

```rust
let store_clone = store.clone();
tokio::spawn(async move {
    store_clone.cleanup().await;
});
// Original `store` still usable here
```

### 9.2 Async vs Sync Locks

| Use Case | Primitive | Why |
|----------|-----------|-----|
| Lock held across `.await` | `tokio::sync::Mutex` | Won't block runtime thread |
| Lock NOT held across `.await` | `std::sync::Mutex` | Cheaper, no async overhead |
| Read-heavy, write-rare | `tokio::sync::RwLock` | Concurrent reads |
| Broadcast to many receivers | `tokio::sync::broadcast` | Fan-out events |
| One-shot response | `tokio::sync::oneshot` | Approval/ask channels |
| Graceful cancellation | `tokio_util::sync::CancellationToken` | Agent run cancellation |

**Critical rule — never hold `std::sync::Mutex` across `.await`:**

```rust
// WRONG — blocks tokio worker thread
let guard = std_mutex.lock().unwrap();
tokio::time::sleep(Duration::from_secs(1)).await;
drop(guard);

// CORRECT — use tokio::sync::Mutex
let guard = tokio_mutex.lock().await;
tokio::time::sleep(Duration::from_secs(1)).await;
drop(guard);
```

### 9.3 Snapshot-Then-Release

If code under a lock needs to do I/O (network, DB, file), copy the data out under the lock, release it, then do the I/O:

```rust
// WRONG — holds lock during I/O
let mut guard = sessions.lock().await;
let session = guard.get(&id).unwrap();
let result = reqwest::get(&session.url).await?;  // network I/O under lock
guard.remove(&id);

// CORRECT — snapshot under lock, release, then I/O
let url = {
    let guard = sessions.lock().await;
    guard.get(&id).map(|s| s.url.clone())
};
if let Some(url) = url {
    let result = reqwest::get(&url).await?;
    let mut guard = sessions.lock().await;
    guard.remove(&id);
}
```

### 9.4 CancellationToken for Agent Runs

Agent runs use `CancellationToken` for graceful cancellation — not `abort()`:

```rust
// crates/server/src/handlers/ws.rs
type ActiveRuns = Arc<Mutex<HashMap<String, CancellationToken>>>;

// Starting a run
let token = CancellationToken::new();
{
    let mut runs = active_runs.lock().await;
    runs.insert(session_id.clone(), token.clone());
}

// Cancelling a run (on "cancel" WebSocket message)
let runs = active_runs.lock().await;
if let Some(token) = runs.get(&session_id) {
    token.cancel();
}
```

**Never use `JoinHandle::abort()`** to cancel agent runs — it doesn't give the runner a chance to clean up.

### 9.5 Broadcast Channel for Events

`ClientHub` uses `tokio::sync::broadcast` for fan-out to all WebSocket clients:

```rust
pub struct ClientHub {
    tx: broadcast::Sender<HubEvent>,
}

impl ClientHub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self { tx }
    }

    pub fn broadcast(&self, event_type: &str, payload: serde_json::Value) {
        let _ = self.tx.send(HubEvent { ... });
    }

    pub fn subscribe(&self) -> broadcast::Receiver<HubEvent> {
        self.tx.subscribe()
    }
}
```

**Buffer size 256** — if a client falls behind by 256 messages, it gets `RecvError::Lagged`. Handle this gracefully (log and continue, don't panic).

### 9.6 No Blocking in Async Context

Never call blocking operations in async handlers without `spawn_blocking`:

```rust
// WRONG — blocking DB call in async handler (r2d2 pool.get() can block)
pub async fn list_chats(State(state): State<AppState>) -> HandlerResult<...> {
    // This is acceptable ONLY because r2d2 has a timeout and our pool is small
    // For truly blocking operations, use spawn_blocking:
}

// CORRECT for heavy computation
let result = tokio::task::spawn_blocking(move || {
    expensive_cpu_work()
}).await?;
```

**Note:** Our r2d2 SQLite pool calls are synchronous but fast (local file, WAL mode, busy_timeout=5000ms). This is acceptable in async handlers for now but should be watched.

### 9.7 Background Task Pattern

Long-running background tasks follow this pattern:

```rust
// Scheduler
tokio::spawn(async move {
    tokio::time::sleep(Duration::from_secs(10)).await;  // initial delay
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    loop {
        interval.tick().await;
        if let Err(e) = tick(&store, &runner, &hub).await {
            warn!("scheduler tick error: {}", e);
        }
    }
});

// Update checker with cancellation
tokio::spawn(async move {
    let checker = updater::BackgroundChecker::new(...);
    checker.run(cancel_token).await;
});
```

**Always handle errors in spawned tasks** — a panic in `tokio::spawn` aborts that task silently.

---

## Audit Checklist

Use this before committing or when reviewing code.

### New Endpoint

- [ ] Handler follows standard pattern (`State`, extractors, `HandlerResult`, `to_error_response`)
- [ ] Route registered in `crates/server/src/lib.rs` in the correct section (auth/public/protected)
- [ ] `cargo build` passes with zero warnings
- [ ] Frontend uses generated import from `$lib/api/nebo`, not manual fetch
- [ ] Handler import added to `handlers/mod.rs` if new module

### Code Reuse

- [ ] Searched existing handlers for similar logic before writing new
- [ ] Uses Axum extractors for all HTTP concerns (no custom parsing)
- [ ] Uses `state.store.*` for all DB access (no raw SQL in handlers)
- [ ] Uses existing types or extends them (no parallel type hierarchies)
- [ ] Uses `to_error_response` for error conversion (no custom error formatting)

### Dead Code

- [ ] No unreferenced handlers remain after feature changes
- [ ] No orphaned types after removing endpoints
- [ ] No commented-out code blocks
- [ ] No `#[allow(dead_code)]` without justification comment
- [ ] No unused imports (compiler catches, but check `#[allow(unused)]`)
- [ ] Removed feature's store methods deleted from `crates/db/src/`
- [ ] `cargo build` clean after removals

### Rust Patterns

- [ ] Errors propagated with `?` and `.map_err(to_error_response)` (not `.unwrap()`)
- [ ] Pattern matching on error variants (not string comparison)
- [ ] `thiserror` `#[from]` for error conversions
- [ ] Handler variables named correctly (`state`, `id`, `body`, `q`, `claims`)
- [ ] Traits ≤7 methods, defined at consumer side
- [ ] `serde_json::json!` for ad-hoc responses, named structs for reused shapes
- [ ] `#[serde(rename_all = "camelCase")]` on response types

### Concurrency

- [ ] `Arc` for every value shared across `tokio::spawn` boundaries
- [ ] `tokio::sync::Mutex` when lock held across `.await` — `std::sync::Mutex` otherwise
- [ ] No I/O (network, DB, file) while holding any lock — use snapshot-then-release
- [ ] `CancellationToken` for agent run cancellation — never `JoinHandle::abort()`
- [ ] Broadcast channel `RecvError::Lagged` handled gracefully (log, don't panic)
- [ ] Background tasks log errors from spawned tasks (never silent failures)
- [ ] No `std::sync::Mutex` held across `.await` points

### API Design

- [ ] Response uses flat format with plural collection key + `total`
- [ ] JSON tags use `camelCase` via `#[serde(rename_all = "camelCase")]`
- [ ] Pagination uses `limit`/`offset` pattern with hard cap
- [ ] Action endpoints use `POST /resource/{id}/verb`
- [ ] DELETE returns message response
- [ ] Optional fields use `Option<T>` + `skip_serializing_if`

### Architecture

- [ ] No competing pathways — functionality exists in ONE place only
- [ ] Boundary awareness — edits respect crate boundaries
- [ ] No new internal wrappers that duplicate a dependency's surface
- [ ] Handler only does: extract → store call → format response
- [ ] All DB access through `state.store.*`
- [ ] No circular crate dependencies
- [ ] No global mutable state — everything through `AppState`
- [ ] No new crate dependency directions that don't already exist
- [ ] WebSocket route stays outside `CompressionLayer`

### Build Verification

- [ ] `cargo build` passes with zero warnings
- [ ] `cargo test` passes (72+ tests across workspace)
- [ ] `cd ../../app && pnpm check` passes (if frontend changed)
- [ ] No new `// TODO` or `// FIXME` without a tracking issue

---

## Key File Inventory

| File | Purpose | When to Touch |
|------|---------|---------------|
| `crates/server/src/lib.rs` | Server setup, route registration, startup logic | Adding/removing routes, middleware |
| `crates/server/src/state.rs` | AppState struct definition | Adding a new shared dependency |
| `crates/server/src/handlers/mod.rs` | HandlerResult, to_error_response, handler module declarations | Adding new handler modules |
| `crates/server/src/handlers/*.rs` | HTTP handlers by domain | Adding/modifying endpoints |
| `crates/server/src/middleware.rs` | JWT auth, rate limiting, security headers | Modifying auth/security behavior |
| `crates/types/src/error.rs` | NeboError enum with status code mapping | Adding new error variants |
| `crates/db/src/store.rs` | Store struct, connection pool | Adding new query methods |
| `crates/db/src/migrate.rs` | Migration runner (goose-compatible) | Rarely |
| `crates/db/migrations/*.sql` | SQL migration files (numbered) | Adding new tables/columns |
| `crates/auth/src/service.rs` | AuthService (login, register, refresh) | Modifying auth flows |
| `crates/auth/src/jwt.rs` | JWT validation/generation | Modifying token behavior |
| `crates/agent/src/runner.rs` | Agentic loop, provider selection | Modifying agent behavior |
| `crates/tools/src/registry.rs` | Tool registration and execution | Adding/modifying tools |
| `crates/tools/src/policy.rs` | Tool approval policy | Modifying tool permissions |
| `crates/ai/src/*.rs` | AI provider implementations | Adding providers, changing API calls |
| `Cargo.toml` (root) | Workspace members and shared dependencies | Adding crates or dependencies |
| `crates/*/Cargo.toml` | Per-crate dependencies | Adding crate-specific dependencies |

---

## Related SME Documents

- **MIGRATION-SME.md** — Master migration index, gap analysis, route/tool mappings, roadmap
- **MEMORY_AND_PROMPT.md** — Memory extraction, prompt assembly, 31-step pipeline
- **workflow-engine.md** — Workflow execution (lean mode), activity model, token budgets
- **platform-taxonomy.md** — Skills, Tools, Workflows, Roles taxonomy spec
- **agent-core.md** — Runner, steering, sessions, skills, AI providers (deep dive)
- **agent-tools.md** — Tool registry, STRAP pattern, 20+ tools (deep dive)
- **lanes-and-hub.md** — Lane system, hub, WebSocket lifecycle (deep dive)
- **browser-and-relay.md** — Chrome/CDP, extension relay (deep dive)
- **mcp-and-comm.md** — MCP server/client/bridge, NeboLoop comm (deep dive)
- **middleware-and-config.md** — All middleware, security, config (deep dive)
- **handlers-cli-db.md** — 60+ handlers, CLI commands, DB layer (deep dive)
