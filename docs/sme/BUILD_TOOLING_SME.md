# BUILD_TOOLING_SME.md

Subject Matter Expert document for Nebo's build tooling systems: the auto-updater, the API code generator (genapi), and the plugin publishing pipeline.

Last updated: 2026-05-15

---

## Table of Contents

1. [Part 1: Auto-Updater](#part-1-auto-updater)
2. [Part 2: API Code Generation (genapi)](#part-2-api-code-generation-genapi)
3. [Part 3: Plugin Publishing Pipeline](#part-3-plugin-publishing-pipeline)
4. [Part 4: CI/CD Release Pipeline](#part-4-cicd-release-pipeline)
5. [Part 5: Makefile Build Targets](#part-5-makefile-build-targets)

---

## Part 1: Auto-Updater

### Source Files

| File | Purpose |
|---|---|
| `crates/updater/src/lib.rs` | Version check, download, checksum verification, background checker |
| `crates/updater/src/apply.rs` | Binary replacement, platform-specific install strategies, rollback |
| `crates/server/src/handlers/agent.rs` | HTTP endpoints: `GET /update/check`, `POST /update/apply` |
| `crates/server/src/lib.rs` | Background checker spawn, auto-download, WS notifications |

### Architecture Overview

```
                         +----------------------------+
                         |  cdn.neboai.com/releases |
                         |                            |
                         |  version.json              |
                         |  v0.5.0/                   |
                         |    nebo-darwin-arm64        |
                         |    nebo-darwin-amd64        |
                         |    nebo-linux-amd64         |
                         |    Nebo-0.5.0-arm64.dmg    |
                         |    Nebo-0.5.0-amd64.msi    |
                         |    checksums.txt            |
                         +------------+---------------+
                                      |
                   +------------------+------------------+
                   |                                     |
           Background Check                      On-Demand Check
           (every 60 min)                     (GET /api/v1/update/check)
                   |                                     |
                   v                                     v
         +-------------------+                 +-------------------+
         | BackgroundChecker |                 | update_check()    |
         | (lib.rs:296)      |                 | handler           |
         |                   |                 +-------------------+
         | 30s boot delay    |
         | polls hourly      |
         +--------+----------+
                  |
                  | update available?
                  v
         +-------------------+         +-----------------------+
         | WS broadcast      |-------->| Frontend notification |
         | "update_available"|         | (toast / banner)      |
         +-------------------+         +-----------------------+
                  |
                  | auto_download (if can_auto_update)
                  v
         +-------------------+
         | download()        |    Progress via WS:
         | (CDN fetch)       |--> "update_progress" events
         +--------+----------+
                  |
                  v
         +-------------------+
         | verify_checksum() |    SHA256 against checksums.txt
         +--------+----------+
                  |
                  v
         +-------------------+
         | Stage binary      |    Stored in update_pending Arc<Mutex>
         | (in memory)       |    WS: "update_ready"
         +--------+----------+
                  |
                  | User triggers POST /api/v1/update/apply
                  v
         +-------------------+
         | apply_update()    |    Detects install method,
         | (apply.rs)        |    dispatches to platform path
         +--------+----------+
                  |
         +-------+--------+--------+
         |                |        |
     app_bundle      direct     homebrew/pkg
     (DMG/MSI/      (execve/   (rejected:
      AppImage)      rename)    can_auto_update=false)
```

### Update Check Mechanism

The updater checks for new versions by fetching a JSON manifest from the CDN.

**CDN endpoints:**
- `https://cdn.neboai.com/releases/version.json` -- latest version pointer
- `https://cdn.neboai.com/releases/{tag}/` -- per-release assets

**version.json format:**
```json
{
  "version": "v0.5.0",
  "release_url": "https://github.com/NeboLoop/nebo/releases/tag/v0.5.0",
  "published_at": "2026-05-15T00:00:00Z"
}
```

**Version comparison logic:**
1. Both versions are normalized (strip leading `v`, trim whitespace).
2. The "dev" version is never updatable (`current != "dev"`).
3. Semver comparison splits on `.` into `[major, minor, patch]` and compares numerically left-to-right.
4. Equal versions are NOT considered "newer" -- only strictly greater counts.

**Install method detection** (`detect_install_method()`):
The updater inspects the current executable path to determine how Nebo was installed. This gates whether auto-update is possible.

| Method | Detection | `can_auto_update` |
|---|---|---|
| `app_bundle` | macOS: path contains `.app/Contents/MacOS/`; Windows: path contains `\Nebo\` or `\WindowsApps\` | true |
| `homebrew` | Path contains `/opt/homebrew/` or `/usr/local/Cellar/` | false |
| `package_manager` | Linux: `dpkg -S <path>` succeeds | false |
| `direct` | Fallback (standalone binary) | true |

Only `direct` and `app_bundle` methods support auto-update. Homebrew and package manager users are told to update via their package manager.

### Background Checker

The `BackgroundChecker` struct runs a periodic check loop:

1. **Boot delay:** 30 seconds after server start (let the app initialize).
2. **Initial check:** Immediately after boot delay.
3. **Periodic checks:** Every 60 minutes (configurable via `interval`).
4. **Dedup:** Tracks `last_notified` version to avoid spamming the same notification.
5. **Cancellation:** Respects a `CancellationToken` for clean shutdown.

When an update is found, the server:
1. Broadcasts `"update_available"` over WebSocket with the `CheckResult`.
2. If `can_auto_update` is true, immediately starts downloading.
3. During download, broadcasts `"update_progress"` events with percentage.
4. After download + checksum verification, stores the path in `update_pending` and broadcasts `"update_ready"`.

### Download and Verification

**Asset naming convention:**

| Platform | Architecture | CLI Binary Name | App Bundle Name |
|---|---|---|---|
| macOS | arm64 | `nebo-darwin-arm64` | `Nebo-{version}-arm64.dmg` |
| macOS | amd64 | `nebo-darwin-amd64` | `Nebo-{version}-amd64.dmg` |
| Linux | arm64 | `nebo-linux-arm64` | `Nebo-{version}-arm64.AppImage` |
| Linux | amd64 | `nebo-linux-amd64` | `Nebo-{version}-amd64.AppImage` |
| Windows | amd64 | `nebo-windows-amd64.exe` | `Nebo-{version}-amd64.msi` |

The Rust arch names are mapped to CDN convention: `aarch64` -> `arm64`, `x86_64` -> `amd64`, `macos` -> `darwin`.

**Download flow:**
1. Detect install method -> choose asset (raw binary vs app bundle).
2. Construct URL: `https://cdn.neboai.com/releases/{tag}/{asset_name}`.
3. Stream download to a temp file (`/tmp/nebo-update-{uuid}`).
4. Report progress via callback `(downloaded_bytes, total_bytes)`.
5. Set executable permission on Unix (`chmod 755`).

**Checksum verification:**
1. Fetch `https://cdn.neboai.com/releases/{tag}/checksums.txt`.
2. Parse format: `<sha256_hex>  <filename>` (standard `sha256sum` output).
3. Find the line matching the downloaded asset name.
4. Compute SHA256 of the downloaded file.
5. Compare hashes (case-insensitive).
6. If `checksums.txt` returns 404, verification is skipped (graceful degradation).

### Installation Process (apply.rs)

The `apply()` function dispatches to two strategies based on install method.

#### Strategy 1: Direct Binary Update (CLI / headless)

**Unix (execve):**
```
1. Resolve current exe path (follow symlinks via canonicalize)
2. Health check: run "nebo --version" on new binary
3. Backup: copy current binary -> current.old
4. Replace: copy new binary -> current path
   - On failure: rollback from .old backup
5. Clean up temp file
6. Run pre-apply hook (release resources, flush state)
7. execve() into new binary with same args + env
   - Process is replaced in-place (no restart visible to user)
   - If execve fails, return error (old binary is already replaced)
```

**Windows (rename + spawn):**
```
1. Health check: run "nebo --version" on new binary
2. Rename current.exe -> current.exe.old
   - Windows allows renaming running executables
3. Copy new binary -> current.exe path
   - On failure: rename .old back
4. Clean up temp
5. Run pre-apply hook
6. Spawn new process with same args (skip arg[0])
7. process::exit(0)
```

#### Strategy 2: App Bundle Update (Desktop)

**macOS (DMG):**
```
1. hdiutil attach -nobrowse -noverify -noautoopen <dmg>
2. Parse mount point from hdiutil output
3. Locate Nebo.app in mounted volume
4. Determine destination (walk ancestors for .app, fallback /Applications)
5. Run pre-apply hook
6. rm -rf destination .app
7. cp -R source Nebo.app -> destination
8. hdiutil detach <mount_point>
9. Clean up DMG temp file
10. Spawn relaunch shell:
    "while kill -0 {pid}; do sleep 0.2; done; open <app_path>"
    - Waits for current process to die before opening new app
    - Avoids race condition with macOS app lifecycle
11. process::exit(0)
```

**Windows (MSI):**
```
1. Run pre-apply hook
2. Spawn: cmd /C "msiexec /i <msi> /quiet /norestart && start <exe>"
   - MSI installer runs silently
   - After install completes, relaunches the exe
3. process::exit(0)
```

**Linux (AppImage):**
```
1. Resolve current exe (follow symlinks)
2. Backup: copy current -> current.old
3. Replace: copy new AppImage -> current path
   - On failure: rollback from .old
4. chmod 755 new binary
5. Clean up temp
6. Run pre-apply hook
7. Spawn new binary as child process
8. process::exit(0)
```

### Rollback Capability

Rollback is built into the direct binary update path:

| Step | Rollback Action |
|---|---|
| Copy new binary fails | Restore from `.old` backup |
| Health check fails | Never replaced -- original still in place |
| execve fails (Unix) | Binary already replaced, `.old` available for manual restore |
| MSI install fails (Windows) | `.exe.old` available for manual restore |
| AppImage copy fails (Linux) | Restore from `.old` backup |

The `.old` suffix backup is never automatically cleaned up -- it persists as a manual recovery point.

### Pre-Apply Hook

The `set_pre_apply_hook()` function registers a callback that runs before the process restarts. This is used by the server to:
- Flush pending database writes
- Close WebSocket connections gracefully
- Release file locks

The hook is stored in a global `Mutex<Option<Box<dyn Fn() + Send>>>`.

---

## Part 2: API Code Generation (genapi)

### Source Files

| File | Purpose |
|---|---|
| `scripts/genapi/main.go` | Entry point, orchestrates parse + emit pipeline |
| `scripts/genapi/parser.go` | Parses Rust structs, routes, handlers, WS events |
| `scripts/genapi/emitter.go` | Generates neboComponents.ts and nebo.ts |
| `scripts/genapi/typemap.go` | Rust-to-TypeScript type mapping, naming conventions |
| `scripts/genapi/overrides.go` | Manual type overrides for complex response shapes |
| `scripts/genapi/go.mod` | Go module definition |

### Single Generator (canonical)

Nebo has exactly one API code generator: the **Go tool** at `scripts/genapi/`. It is itself written in Go, but it produces **only TypeScript** — it parses the Rust backend (structs, routes, handlers, WS events) and emits `neboComponents.ts` (interfaces) and `nebo.ts` (API functions). This guarantees a source-of-truth contract between the Rust backend and the SvelteKit frontend, since the types are inferred directly from Rust rather than hand-maintained.

Run it with:

```
make gen        # == cd scripts/genapi && go run .
```

A second, catalog-driven TypeScript generator (`app/scripts/genapi.ts` + `app/scripts/api-catalog.json`, npm script `gen:api`) used to exist but was **removed** — it required a hand-maintained catalog that drifted from the real Rust routes, so it could not guarantee the contract. Do not reintroduce a competing generator: the Go tool is the only supported path.

### Architecture Overview (Go Generator)

```
+---------------------+     +---------------------+     +---------------------+
| Rust Source Code     |     | Go Parser           |     | Go Emitter          |
|                      |     | (parser.go)         |     | (emitter.go)        |
| crates/db/src/       |     |                     |     |                     |
|   models.rs          +---->| scanStructs()       +---->| generateComponents()|
|   queries/*.rs       |     |   - Serialize derive |     |   neboComponents.ts |
|                      |     |   - serde attrs      |     |                     |
| crates/types/src/    |     |                     |     |                     |
|   *.rs               +---->| scanRoutes()        +---->| generateAPI()       |
|                      |     |   - .route() calls   |     |   nebo.ts           |
| crates/server/src/   |     |                     |     |                     |
|   routes/*.rs        +---->| scanHandlers()      |     |                     |
|   handlers/*.rs      |     |   - json!({}) bodies |     |                     |
|                      |     |   - Query<T> params  |     |                     |
+---------------------+     |                     |     +---------------------+
                             | scanStoreMethodTypes|
                             |   - return types    |         +----------------+
                             |                     |         | typemap.go     |
                             | scanWSEvents()      |         | Rust -> TS     |
                             |   - broadcast()     |         | type mapping   |
                             |   - client messages |         +----------------+
                             +---------------------+
                                                             +----------------+
                                                             | overrides.go   |
                                                             | Manual type    |
                                                             | corrections    |
                                                             +----------------+
```

### Pipeline Steps (Go Generator)

The `main()` function in `main.go` executes 7 sequential steps:

**Step 1: Parse Rust Structs** (`scanStructs`)
- Scans directories: `crates/db/src/`, `crates/types/src/`, `crates/server/src/handlers/`
- Finds structs with `#[derive(...Serialize...)]`
- Extracts fields, serde attributes (`rename`, `skip`, `default`, `serialize_with`)
- Tracks `rename_all` container attribute (e.g., `camelCase`)

**Step 2: Parse Routes** (`scanRoutes`)
- Scans `crates/server/src/routes/*.rs`
- Matches `.route("path", axum::routing::method(handler))` patterns
- Extracts HTTP method, path, handler function reference, source file

**Step 3: Parse Store Method Types** (`scanStoreMethodTypes`)
- Scans `crates/db/src/queries/*.rs`
- Extracts `pub fn method_name(...) -> Result<ReturnType, ...>` signatures
- Unwraps `Result<T, E>` to get the inner return type `T`
- Used later to infer handler response types from store method calls

**Step 4: Parse Deserialize Structs** (`scanDeserializeStructs`)
- Scans handler files for structs with `#[derive(...Deserialize...)]`
- These are `Query<T>` parameter types (not emitted as TS interfaces)
- Keyed by `filename::StructName` to handle name collisions

**Step 5: Parse Handler Responses** (`scanHandlers`)
- Splits each handler file into individual functions by tracking brace depth
- For each function:
  - Extracts `Query<T>` parameters from function signature
  - Builds variable type map from `let x: Type = ...` bindings
  - Traces `state.store.method()` calls to resolve variable types via store method return types
  - Finds the last `json!({...})` macro call (the response body)
  - Extracts top-level key-value pairs from the json macro
  - Infers TS types using a 4-tier priority system

**Step 6: Parse WebSocket Events** (`scanWSEvents`)
- Scans `handlers/ws.rs` for `broadcast("event_type", ...)` calls
- Extracts event payloads from nearby `json!({...})` macros
- Also finds client message types from match arm patterns

**Step 7: Generate Output Files**
- `neboComponents.ts`: Struct interfaces + handler response types + common types + WS events
- `nebo.ts`: Typed API functions calling `webapi.get/post/put/delete/patch`

### Type Inference System

The Go generator uses a 4-tier priority system to infer TypeScript types for response keys:

```
Priority 1: Explicit overrides (overrides.go)
    |
    v
Priority 2: Variable type resolution (let bindings + store method tracing)
    |
    v
Priority 3: High-confidence expression patterns (literals, .len(), .is_empty())
    |
    v
Priority 4: Key-name heuristics (arrayKeys, stringKeys, numberKeys, booleanKeys)
    |
    v
Fallback: Expression suffix patterns (ends with "_id" -> string, etc.)
    |
    v
Default: "unknown"
```

**Variable type resolution** is the most sophisticated step:

1. Direct `let var: Type = ...` annotations are extracted.
2. `state.store.method_name(...)` calls are traced: the method's return type from `crates/db/src/queries/` is assigned to the variable.
3. Three strategies handle different code layouts:
   - Direct assignment: `let x = state.store.get_agents(...)`
   - Conditional: `let x = if ... { state.store.method() } else { ... }`
   - Multiline chain: `let x = state\n.store\n.method()`
4. Transforming operations (`.len()`, `.map()`, `.filter()`, etc.) void the store type inference.

**Key-name heuristics** use curated maps of known field names:

| Map | Examples | Inferred TS Type |
|---|---|---|
| `arrayKeys` | agents, chats, messages, runs, memories, plugins | `unknown[]` |
| `stringKeys` | message, error, status, id, version, token, email | `string` |
| `numberKeys` | total, count, offset, limit, port, pid, uptime | `number` |
| `booleanKeys` | success, ok, connected, authenticated, installed | `boolean` |
| `objectKeys` | agent, chat, session, profile, config, settings | `unknown` |

### Type Mapping (typemap.go)

The `rustTypeToTS()` function handles recursive type conversion:

| Rust Type | TypeScript Type |
|---|---|
| `String`, `&str`, `Cow<'_, str>` | `string` |
| `bool` | `boolean` |
| `i8..i128`, `u8..u128`, `f32`, `f64`, `usize`, `isize` | `number` |
| `()` | `void` |
| `serde_json::Value` | `unknown` |
| `Option<T>` | unwrapped to `T` (field gets `?` modifier) |
| `Vec<T>` | `T[]` |
| `HashMap<K, V>`, `BTreeMap<K, V>` | `Record<K, V>` |
| `Box<T>`, `Arc<T>`, `Rc<T>`, `Mutex<T>`, `RwLock<T>` | unwrapped to `T` |
| Known struct name | Used as-is (interface reference) |

**Naming conventions:**
- Rust `snake_case` -> TS `camelCase` (for fields with `rename_all = "camelCase"`)
- Handler `handlers::module::func_name` -> TS `funcName` (via `snakeToCamel`)
- Prefixed modules: `handlers::neboai::account_status` -> `neboAIAccountStatus`
- Response types: `funcName` -> `FuncNameResponse`

**Custom serializer mapping** (`resolveSerializeWith`):
- `*i64_as_bool*` / `*as_bool*` -> `boolean`
- `*json_string_as_array*` -> `string[]`

### Type Overrides (overrides.go)

For response shapes the generator cannot infer (ad-hoc json objects, transformed collections), explicit overrides are defined:

```go
var typeOverrides = map[string]string{
    "list_agent_chats.chats":        "EnrichedChat[]",
    "get_active_agents.agents":      "ActiveAgent[]",
    "get_chat_messages.messages":    "ChatMessage[]",
    "get_commander_org.nodes":       "CommanderNode[]",
    // ... ~15 total overrides
}
```

When an override references a type that does not exist as a Rust struct, it is defined in `extraInterfaces`:

```go
var extraInterfaces = map[string]string{
    "EnrichedChat": `export interface EnrichedChat {
        id: string
        name: string
        // ...
    }`,
    // ... ~10 manually defined interfaces
}
```

### Input/Output File Mapping

**Inputs (parsed from Rust):**
```
crates/db/src/*.rs                    -> struct definitions
crates/types/src/*.rs                 -> struct definitions
crates/server/src/handlers/*.rs       -> struct definitions + handler responses
crates/server/src/routes/*.rs         -> route definitions
crates/db/src/queries/*.rs            -> store method return types
```

**Outputs (TypeScript):**
```
app/src/lib/api/neboComponents.ts     -> TypeScript interfaces + response types + WS events
app/src/lib/api/nebo.ts               -> Typed API functions
```

### How to Run

```bash
# Via Makefile (recommended) -- runs the Go generator (parses Rust, emits TypeScript)
make gen

# Directly (equivalent)
cd scripts/genapi && go run .
```

### Generated Code Structure

**neboComponents.ts** (generated by Go tool):
```typescript
// Code generated by scripts/genapi. DO NOT EDIT.
// To regenerate: make gen

// -- Rust Struct Models --
export interface Agent {
    id: string
    name: string
    slug: string
    // ... fields from Rust struct with Serialize derive
}

// -- API Response Types (inferred from handlers) --
export interface ListAgentsResponse {
    agents: Agent[]
    total: number
}

// -- Common Types --
export interface ErrorResponse { error: string }
export interface MessageResponse { message: string }
export interface SuccessResponse { success: boolean }

// -- Override Types (see scripts/genapi/overrides.go) --
export interface EnrichedChat { /* ... */ }

// -- WebSocket Event Types --
export type WSServerEventType =
    | "chat_message"
    | "update_available"
    | "update_progress"
    // ...

export type WSClientMessageType =
    | "chat"
    | "cancel"
    | "auth"
    // ...
```

**nebo.ts** (generated by scripts/genapi):
```typescript
// Code generated by scripts/genapi. DO NOT EDIT.
import webapi from "./gocliRequest"
import * as components from "./neboComponents"
export * from "./neboComponents"

/**
 * @description "List agents"
 */
export function listAgents() {
    return webapi.get<components.ListAgentsResponse>(`/api/v1/agents`)
}

/**
 * @description "Get agent"
 */
export function getAgent(id: string) {
    return webapi.get<components.GetAgentResponse>(`/api/v1/agents/${id}`)
}

/**
 * @description "Create chat"
 */
export function createChat(id: string, req: Record<string, unknown> = {}) {
    return webapi.post<components.CreateChatResponse>(`/api/v1/agents/${id}/chats`, req)
}
```

---

## Part 3: Plugin Publishing Pipeline

### Source Files

| File | Purpose |
|---|---|
| `scripts/publish-plugins.sh` | Build, publish, bundle CLI for plugins |
| `crates/napp/src/napp.rs` | .napp envelope format, extraction, validation |
| `crates/napp/src/signing.rs` | ED25519 signing, verification, revocation |
| `crates/napp/src/sealed.rs` | AES-256-GCM encryption for sealed .napp files |
| `crates/napp/src/reader.rs` | .napp archive reader, partial extraction |
| `crates/napp/src/manifest.rs` | Manifest schema and validation |

### Architecture Overview

```
+-------------------+     +-------------------+     +-------------------+
| Plugin Source Repo |     | publish-plugins.sh|     | NeboAI API      |
|                    |     |                   |     |                   |
| repos/plugins/gws/ |     | build             |     | Sign & Package    |
|   Cargo.toml       +---->| (cargo build      +---->| ED25519 sign      |
|   plugin.json      |     |  --release)       |     | Create .napp      |
|   PLUGIN.md        |     |                   |     | envelope           |
|   skills/          |     | publish           |     |                   |
|                    |     | (upload to API)   |     | Optional:          |
+-------------------+     |                   |     | AES-256-GCM seal  |
                           | bundle            |     | (paid plugins)    |
                           | (copy .napp to    |     |                   |
                           |  bundled-napps/)  |     +--------+----------+
                           +-------------------+              |
                                                              v
                                                   +-------------------+
                                                   | CDN / Marketplace |
                                                   |                   |
                                                   | plugins/gws/      |
                                                   |   darwin-arm64.napp|
                                                   |   darwin-amd64.napp|
                                                   |   linux-amd64.napp |
                                                   |   windows-amd64.napp|
                                                   +-------------------+
                                                              |
                                                              v
                                                   +-------------------+
                                                   | Nebo Desktop App  |
                                                   |                   |
                                                   | unwrap_napp()     |
                                                   | verify_signatures()|
                                                   | extract_napp()    |
                                                   +-------------------+
```

### Plugin Registry

The `publish-plugins.sh` script maintains two plugin lists:

**MUST_BUNDLE** (shipped with desktop app):
| Slug | CLI Crate | Platforms |
|---|---|---|
| `gws` | google-workspace-cli | all |
| `digest` | digest-cli | all |
| `nebo-pdf` | nebo-pdf | all |
| `nebo-office` | cli | all |
| `email` | email-cli | all |
| `peek` | peek-cli | darwin only |
| `imessage` | imessage-cli | darwin only |
| `reminders` | reminders-cli | all |
| `watchdog` | watchdog-cli | all |

**SHOULD_BUNDLE** (optional, included when available):
| Slug | CLI Crate | Platforms |
|---|---|---|
| `social` | social-cli | all |
| `devlink` | devlink-cli | all |
| `imagegen` | imagegen-cli | all |
| `ffmpeg` | ffmpeg-cli | all |
| `slack` | slack-cli | all |

### Build Process

The `build` command compiles plugin binaries:

```bash
./scripts/publish-plugins.sh build          # All plugins
./scripts/publish-plugins.sh build gws      # Single plugin
```

**Platform targets:**

| Platform Label | Rust Target Triple |
|---|---|
| `darwin-arm64` | `aarch64-apple-darwin` |
| `darwin-amd64` | `x86_64-apple-darwin` |
| `linux-arm64` | `aarch64-unknown-linux-gnu` |
| `linux-amd64` | `x86_64-unknown-linux-gnu` |
| `windows-amd64` | `x86_64-pc-windows-msvc` |

**Build flow per plugin:**
1. Locate plugin source at `$REPOS_DIR/$slug` (default: `../repos/plugins/$slug`).
2. Expand platform spec: `all` -> 5 targets, `darwin` -> 2 targets.
3. For each platform:
   - Skip cross-compilation unless running on target platform (CI handles cross builds).
   - Run `cargo build --release -p $cli_crate --target $target` in the plugin repo.
   - Look up binary name from `plugin.json` -> `platforms.$platform.binaryName`.
   - Verify binary exists and report size.

### .napp Envelope Format

The `.napp` file format is a custom binary envelope wrapping a tar.gz archive:

```
+------+----+----------+----------+-------------------+
| NAPP | v1 | ED25519  | SHA256   | tar.gz payload    |
| 4B   | 1B | sig 64B  | hash 32B | variable length   |
+------+----+----------+----------+-------------------+
 ^      ^    ^          ^          ^
 |      |    |          |          Inner archive contents:
 |      |    |          |            manifest.json
 |      |    |          |            binary (or named binary)
 |      |    |          |            signatures.json
 |      |    |          |            PLUGIN.md / SKILL.md
 |      |    |          |            skills/ (optional)
 |      |    |          |            ui/ (optional)
 |      |    |          |
 |      |    |          SHA256 of payload
 |      |    |          (integrity check)
 |      |    |
 |      |    ED25519 signature of (hash + payload)
 |      |    (proves NeboAI signed it)
 |      |
 |      Version byte (0x01)
 |
 Magic bytes: "NAPP"
```

**Header size:** 101 bytes (4 magic + 1 version + 64 signature + 32 hash).

**Verification order:**
1. Check magic bytes (`NAPP`).
2. Check version byte (must be `0x01`).
3. Verify SHA256 hash of payload (cheap integrity check).
4. Verify ED25519 signature of `(hash || payload)` using NeboAI's public key.

### Signing and Verification

**Key management:**

| Key | Location | Purpose |
|---|---|---|
| NeboAI private key | Server-side only | Signs .napp envelopes |
| NeboAI public key | `crates/napp/neboai_public_key.bin` (compile-time embedded) | Offline verification |
| NeboAI public key | `GET /api/v1/apps/signing-key` (runtime fetch) | Online verification with key rotation |

The embedded public key enables offline verification at first launch (no network needed). The `SigningKeyProvider` caches the runtime key for 24 hours.

**Signature verification (`verify_signatures`):**
In addition to the envelope signature, .napp archives contain `signatures.json` with per-file signatures:

```json
{
  "manifest_signature": "<base64 ED25519 sig of manifest.json>",
  "binary_hash": "<hex SHA256 of binary>",
  "binary_signature": "<base64 ED25519 sig of binary>"
}
```

Verification checks:
1. Verify manifest signature: `key.verify(manifest_data, manifest_sig)`.
2. Verify binary hash: `SHA256(binary) == binary_hash`.
3. Verify binary signature: `key.verify(binary_data, binary_sig)`.

**Revocation checking:**
The `RevocationChecker` fetches a revocation list from `GET /api/v1/apps/revocations` (cached 1 hour). Revoked app IDs are blocked from installation. On network failure, it fails open (allows installation).

### Sealed .napp Files (Paid Plugins)

Paid plugins use an additional encryption layer (AES-256-GCM) on top of the .napp envelope:

```
.napp envelope
  |
  v
+-------------------+           +-------------------+
| Sealed payload    |   key     | Plain tar.gz      |
| [12B nonce]       | -------> | (decrypted in      |
| [AES-256-GCM      |  unseal  |  memory only)     |
|  ciphertext + tag] |          |                    |
+-------------------+           +-------------------+
```

**Key derivation:**
```
HKDF-SHA256(
  master_secret = license_key,
  salt = artifact_id,
  info = "neboai-license-v1"
) -> 32-byte AES key
```

- Each artifact gets a unique derived key (same master secret, different artifact IDs).
- License scope (user/bot) is NOT part of derivation -- authorization is server-side.
- Sealed .napp files never need re-download on license transfer.

**Sealed vs plain detection:**
Plain payloads start with gzip magic (`0x1f 0x8b`). Sealed payloads start with a random 12-byte nonce. The `is_sealed()` function checks the first two bytes.

**Partial extraction for sealed plugins:**
Sealed plugins use `partial_extract_sealed_napp()` to extract only executables and metadata to disk. Intellectual property (SKILL.md, references, assets) stays inside the sealed .napp and is read in memory at runtime via `read_sealed_napp_entry()`.

Extracted entries: `binary`, `app`, `scripts/*`, `bin/*`, `manifest.json`, `plugin.json`, `signatures.json`.
Kept sealed: `SKILL.md`, `PLUGIN.md`, `skills/`, `ui/`, other content.

### .napp Extraction Security

The `extract_napp()` function enforces multiple security checks:

| Check | Purpose |
|---|---|
| Path traversal (`..`, leading `/`) | Prevent writing outside dest dir |
| Symlink/hardlink rejection | Prevent symlink attacks |
| Allowlisted filenames | Only known file types extracted |
| Size limits: binary 500MB, UI 5MB, metadata 1MB | Prevent zip bombs |
| Canonical path verification | Double-check dest stays within dir |
| Binary format validation | Only native executables (ELF, Mach-O, PE) |
| Shebang rejection | No script execution via plugins |
| Defense-in-depth size recheck | Verify actual content size after read |

**Binary format validation:**
Checks magic bytes to ensure the binary is a compiled native executable:
- ELF: `7f 45 4c 46`
- Mach-O 32/64: `fe ed fa ce/cf` (both endians)
- Universal: `ca fe ba be`
- PE/COFF: `4d 5a`
- Shebang scripts (`#!`): explicitly rejected

### Publish Flow

```bash
./scripts/publish-plugins.sh publish        # All plugins
./scripts/publish-plugins.sh publish gws    # Single plugin
```

The publish command is informational -- it prints what WOULD be published:
1. Reads `plugin.json` for version.
2. Lists available binaries per platform.
3. Directs user to NeboAI MCP tools or web dashboard for actual publishing.

After upload, NeboAI's server-side pipeline:
1. Signs the manifest and binary with the NeboAI ED25519 private key.
2. Creates `signatures.json`.
3. Packages everything into a tar.gz archive.
4. Wraps in the .napp envelope (magic + version + signature + hash + payload).
5. Optionally seals with AES-256-GCM for paid plugins.
6. Uploads to CDN per platform.

### Bundle Flow

```bash
./scripts/publish-plugins.sh bundle
```

Copies signed `.napp` files from plugin repos into `src-tauri/bundled-napps/plugins/` for inclusion in the desktop app build. Only copies `.napp` files (not raw binaries) -- the plugin must be published first.

---

## Part 4: CI/CD Release Pipeline

### Source File

| File | Purpose |
|---|---|
| `.github/workflows/release.yml` | Tag-triggered multi-platform build + release |
| `scripts/nebo.rb.tmpl` | Homebrew cask template |

### Trigger

Push a git tag matching `v*` (e.g., `git tag v0.5.0 && git push --tags`).

### Pipeline Architecture

```
                          git push v0.5.0
                                |
                                v
                    +------------------------+
                    |    release.yml          |
                    |    triggered by v* tag  |
                    +------------------------+
                                |
                +---------------+---------------+
                |                               |
                v                               v
        +--------------+               +--------------+
        | frontend     |               | (parallel)   |
        | (ubuntu)     |               |              |
        | pnpm build   |               |              |
        +--------------+               |              |
                |                       |              |
        upload artifact                 |              |
        "frontend-build"                |              |
                |                       |              |
    +-----------+-----------+-----------+-----------+
    |                       |                       |
    v                       v                       v
+-------------------+ +-------------------+ +-------------------+
| build-macos       | | build-linux       | | build-windows     |
| matrix:           | | matrix:           | | windows-latest    |
|   arm64 + amd64   | |   amd64 + arm64   | |                   |
|                   | |                   | |                   |
| 1. Import cert    | | 1. Install deps   | | 1. Install WiX    |
| 2. Download .napp | | 2. Download .napp | | 2. Download .napp |
| 3. cargo tauri    | | 3. cargo tauri    | | 3. cargo tauri    |
|    build          | |    build          | |    build          |
| 4. Re-sign .app   | | 4. Extract binary | | 4. Extract .exe   |
| 5. Create .dmg    | | 5. Build headless | |    + .msi         |
| 6. Notarize .dmg  | |    CLI            | |                   |
+--------+----------+ | 6. Collect .deb   | +--------+----------+
         |             +--------+----------+          |
         |                      |                     |
         v                      v                     v
+-------------------+  +-------------------+  +-------------------+
| Artifacts:        |  | Artifacts:        |  | Artifacts:        |
| nebo-darwin-arm64 |  | nebo-linux-amd64  |  | Nebo-*-setup.exe  |
| nebo-darwin-amd64 |  | nebo-linux-arm64  |  | *.msi             |
| Nebo-*-arm64.dmg  |  | *-headless        |  +--------+----------+
| Nebo-*-amd64.dmg  |  | *.deb             |           |
+--------+----------+  +--------+----------+           |
         |                      |                      v
         |                      |            +-------------------+
         |                      |            | sign-windows      |
         |                      |            | (if enabled)      |
         |                      |            | Azure Trusted     |
         |                      |            | Signing Action    |
         |                      |            | SHA256 Authenticode|
         |                      |            +--------+----------+
         |                      |                     |
         +----------+-----------+---------------------+
                    |
                    v
         +-------------------+
         | release           |
         | (ubuntu)          |
         |                   |
         | 1. Download all   |
         | 2. Prefer signed  |
         |    over unsigned  |
         | 3. Generate       |
         |    checksums.txt  |
         | 4. Upload to CDN  |
         |    (DO Spaces)    |
         | 5. Create GitHub  |
         |    Release        |
         +--------+----------+
                  |
         +-------+--------+
         |                |
         v                v
+-------------------+ +-------------------+
| update-homebrew   | | update-apt        |
| Cross-repo PR to  | | Cross-repo push   |
| neboai/         | | to neboloop/apt   |
| homebrew-tap      | |                   |
|                   | | dpkg-scanpackages |
| envsubst          | | GPG sign Release  |
| nebo.rb.tmpl ->   | |                   |
| Casks/nebo.rb     | |                   |
+-------------------+ +-------------------+
```

### Build Matrix

| Job | Runner | Targets | Outputs |
|---|---|---|---|
| `frontend` | ubuntu-latest | N/A | `app/build/` (SvelteKit prod build) |
| `build-macos` | macos-latest (x2) | aarch64-apple-darwin, x86_64-apple-darwin | Signed .app, .dmg, bare binary |
| `build-linux` | ubuntu-latest, ubuntu-24.04-arm | native | Desktop binary, headless CLI, .deb |
| `build-windows` | windows-latest | native | .exe, .msi |
| `sign-windows` | windows-latest | N/A | Authenticode-signed .exe + .msi |
| `release` | ubuntu-latest | N/A | GitHub Release + CDN upload |
| `update-homebrew` | ubuntu-latest | N/A | Homebrew cask update |
| `update-apt` | ubuntu-latest | N/A | APT repository update |

### macOS Code Signing

The macOS build performs inside-out code signing:

1. **Import certificate:** Decode base64 P12, create temp keychain, import cert.
2. **Build:** `cargo tauri build --target $target`.
3. **Sign frameworks/dylibs:** `codesign --force --sign "$SIGN_IDENTITY" --timestamp --options runtime` on each framework.
4. **Sign main executable:** With `--identifier dev.neboai.nebo --entitlements nebo.entitlements`.
5. **Sign .app bundle:** Same identity and entitlements.
6. **Verify:** `codesign --verify --deep --strict`.
7. **Create DMG:** Using `create-dmg` with drag-to-Applications layout.
8. **Notarize DMG:** Submit via `xcrun notarytool submit --wait`, then `xcrun stapler staple`.

### CDN Upload

Release assets are uploaded to DigitalOcean Spaces (S3-compatible):

```
s3://neboai/releases/version.json          <- latest version pointer
s3://neboai/releases/v0.5.0/
    nebo-darwin-arm64
    nebo-darwin-amd64
    nebo-linux-amd64
    nebo-linux-arm64
    nebo-linux-amd64-headless
    nebo-linux-arm64-headless
    Nebo-0.5.0-arm64.dmg
    Nebo-0.5.0-amd64.dmg
    Nebo-0.5.0-amd64.msi
    Nebo-0.5.0-setup.exe
    checksums.txt
    version.json                               <- per-tag version
```

The `version.json` at the root is what the auto-updater checks. It is overwritten on every release to point to the latest version.

### Homebrew Cask Update

Template (`scripts/nebo.rb.tmpl`):
```ruby
cask "nebo" do
  version "${PKG_VERSION}"
  on_arm do
    url "https://github.com/NeboLoop/nebo/releases/download/${VERSION}/Nebo-${PKG_VERSION}-arm64.dmg"
    sha256 "${SHA_DARWIN_ARM64}"
  end
  on_intel do
    url "https://github.com/NeboLoop/nebo/releases/download/${VERSION}/Nebo-${PKG_VERSION}-amd64.dmg"
    sha256 "${SHA_DARWIN_AMD64}"
  end
  name "Nebo"
  desc "AI agent with web UI - your personal AI companion"
  homepage "https://neboai.com"
  app "Nebo.app"
  binary "#{appdir}/Nebo.app/Contents/MacOS/Nebo"
end
```

`envsubst` replaces `${PKG_VERSION}`, `${VERSION}`, `${SHA_DARWIN_ARM64}`, `${SHA_DARWIN_AMD64}` with computed values, then pushes to `neboloop/homebrew-tap`.

### APT Repository Update

1. Copy `.deb` packages to `pool/main/`.
2. Generate `Packages` index with `dpkg-scanpackages`.
3. Generate `Release` file with `apt-ftparchive release`.
4. GPG-sign: `Release.gpg` (detached) and `InRelease` (clearsign).
5. Push to `neboloop/apt` repo.

---

## Part 5: Makefile Build Targets

### Source File

| File | Purpose |
|---|---|
| `Makefile` | Top-level build orchestration |

### Target Reference

| Target | Command | Description |
|---|---|---|
| `gen` | `cd scripts/genapi && go run .` | Generate TS API client from Rust routes |
| `dev` | `cargo tauri dev` | Hot reload desktop (Tauri + Vite HMR) |
| `run` | `cargo tauri dev --no-watch` | Desktop without Rust file watching |
| `build` | `cargo build --release -p nebo-cli` | Build headless CLI binary |
| `build-desktop` | `pnpm build && cargo tauri build` | Full desktop app (depends on `bundle-napps`) |
| `test` | `cargo test` | Run all workspace tests |
| `clean` | `rm -rf target/ dist/` | Clean build artifacts |
| `seed-plugins` | (inline loop) | Copy plugin binaries from sibling repos to `~/.nebo/nebo/plugins/` |
| `plugin-status` | `scripts/publish-plugins.sh status` | Show build/bundle status of all plugins |
| `bundle-napps` | (creates dirs) | Prepare `src-tauri/bundled-napps/` for desktop build |
| `release` | `clean + release-{darwin,linux,windows}` | Build all platforms |
| `release-darwin` | `cargo tauri build --target {arm64,amd64}` | macOS both architectures |
| `release-linux` | `cargo tauri build + cargo build -p nebo-cli` | Linux desktop + headless |
| `release-windows` | `cargo tauri build` | Windows .exe + .msi |
| `app-bundle` | codesign + Developer ID | Re-sign .app for distribution |
| `dmg` | `create-dmg` or `hdiutil create` | Create .dmg installer |
| `notarize` | `xcrun notarytool submit --wait && stapler staple` | Notarize with Apple |
| `install` | notarize + cp to /Applications | Full pipeline to install locally |
| `github-release` | `gh release create $TAG` | Create GitHub release |

### Dependency Chain

```
install
  |
  v
notarize
  |
  v
dmg
  |
  v
app-bundle
  |
  v
build-desktop
  |
  +--- bundle-napps (prepare .napp directory)
  +--- cd app && pnpm build (frontend)
  +--- cargo tauri build (backend + desktop wrapper)
```

### Plugin Seeding (Development)

The `seed-plugins` target copies plugin binaries from local development repos into the Nebo plugin directory:

```
$REPOS_DIR/$slug/
  plugin.json             -> ~/.nebo/nebo/plugins/$slug/$version/plugin.json
  target/release/$binary  -> ~/.nebo/nebo/plugins/$slug/$version/$binary
  skills/                 -> ~/.nebo/nebo/plugins/$slug/$version/skills/
```

It reads `plugin.json` to determine the version and platform-specific binary name. A restart of Nebo is required to pick up newly seeded plugins.

---

## Appendix: Key Constants

| Constant | Value | Location |
|---|---|---|
| CDN version URL | `https://cdn.neboai.com/releases/version.json` | `updater/src/lib.rs` |
| CDN download base | `https://cdn.neboai.com/releases` | `updater/src/lib.rs` |
| Update check timeout | 5 seconds | `updater/src/lib.rs` |
| Download timeout | 600 seconds (10 min) | `updater/src/lib.rs` |
| Background check interval | 3600 seconds (1 hour) | `server/src/lib.rs` |
| Background check boot delay | 30 seconds | `updater/src/lib.rs` |
| .napp magic bytes | `NAPP` (4 bytes) | `napp/src/napp.rs` |
| .napp version | `0x01` | `napp/src/napp.rs` |
| .napp header size | 101 bytes | `napp/src/napp.rs` |
| Max binary size | 500 MB | `napp/src/napp.rs` |
| Max UI file size | 5 MB | `napp/src/napp.rs` |
| Max metadata size | 1 MB | `napp/src/napp.rs` |
| Signing key cache TTL | 24 hours | `napp/src/signing.rs` |
| Revocation cache TTL | 1 hour | `napp/src/signing.rs` |
| HKDF info string | `neboai-license-v1` | `napp/src/sealed.rs` |
| AES nonce size | 12 bytes | `napp/src/sealed.rs` |
| AES key size | 32 bytes | `napp/src/sealed.rs` |
| Rust version (CI) | 1.88 | `.github/workflows/release.yml` |
| Node version (CI) | 20 | `.github/workflows/release.yml` |
