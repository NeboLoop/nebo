# CONFIG_SYSTEM_SME.md

Subject Matter Expert document for the Nebo Configuration System.

Last updated: 2026-05-15

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Config Loading Pipeline](#2-config-loading-pipeline)
3. [Config File Hierarchy and Merge Strategy](#3-config-file-hierarchy-and-merge-strategy)
4. [The `nebo.yaml` Embedded Config](#4-the-neboyaml-embedded-config)
5. [The `settings.json` User Config](#5-the-settingsjson-user-config)
6. [Models Configuration (`models.yaml`)](#6-models-configuration-modelsyaml)
7. [Environment Variable Overrides](#7-environment-variable-overrides)
8. [Data Directory System](#8-data-directory-system)
9. [Key Structs and Fields](#9-key-structs-and-fields)
10. [Serde Deserialization Patterns](#10-serde-deserialization-patterns)
11. [Default Values and Fallback Behavior](#11-default-values-and-fallback-behavior)
12. [Cross-System Interactions](#12-cross-system-interactions)
13. [CLI Detection System](#13-cli-detection-system)
14. [Bot Identity System](#14-bot-identity-system)
15. [Setup Lifecycle](#15-setup-lifecycle)
16. [Artifact Directory Layout](#16-artifact-directory-layout)
17. [Configuration Hot-Reload](#17-configuration-hot-reload)
18. [Error Handling](#18-error-handling)
19. [Security Considerations](#19-security-considerations)
20. [Data Directory Migration](#20-data-directory-migration)
21. [Testing Config](#21-testing-config)
22. [Dependencies](#22-dependencies)

---

## 1. Architecture Overview

The `nebo-config` crate (`crates/config/`) is the single source of truth for
application configuration. It provides three distinct configuration subsystems:

1. **Static config** (`Config`) -- loaded from embedded `etc/nebo.yaml` at compile time
2. **Local settings** (`Settings`) -- auto-generated secrets in `~/.nebo/settings.json`
3. **Model catalog** (`ModelsConfig`) -- AI model definitions from `models.yaml`

Plus supporting systems for data directory management, CLI tool detection, and
bot identity.

```
crates/config/
  src/
    lib.rs            Public re-exports (Config, Settings, ModelsConfig, defaults)
    config.rs         Config struct, YAML loading, env var expansion, defaults
    settings.rs       Settings struct, load/save, secret generation
    models.rs         ModelsConfig, ModelDef, routing, aliases, CLI providers
    defaults.rs       Data dir resolution, bot_id, artifact paths, setup lifecycle
    cli_detect.rs     CLI tool detection (claude, codex, gemini), PATH augmentation
    models.yaml       Embedded AI model catalog (compile-time included)
  Cargo.toml          Dependencies: serde, serde_yaml, serde_json, shellexpand, etc.
```

### Crate Dependency Position

```
                           +------------------+
                           |   types/         |
                           |  (constants.rs)  |
                           +--------+---------+
                                    |
                           +--------v---------+
                           |   config/        |
                           |  (Config,        |
                           |   Settings,      |
                           |   ModelsConfig,  |
                           |   defaults)      |
                           +--------+---------+
                                    |
              +---------------------+---------------------+
              |                     |                     |
     +--------v--------+  +--------v--------+   +--------v--------+
     |    auth/         |  |    server/      |   |    tools/       |
     | (AuthService)    |  | (AppState,      |   | (Registry)      |
     |                  |  |  handlers,      |   |                 |
     +--------+---------+  |  middleware)    |   +-----------------+
              |            +--------+--------+
              |                     |
              +----------+----------+
                         |
                +--------v--------+
                |    db/          |
                | (Store)         |
                +-----------------+
```

The `config` crate depends only on `types` (for constants and error types). All
other crates depend on `config` for configuration access.

---

## 2. Config Loading Pipeline

The loading pipeline differs between the three config subsystems. Here is the
complete flow from process entry to a fully-initialized `AppState`:

```
PROCESS START (cli/main.rs or src-tauri/main.rs)
  |
  |  1. dotenvy::dotenv().ok()          Load .env file (if present)
  |
  |  2. Config::load_embedded()          Deserialize etc/nebo.yaml
  |     |
  |     +-- include_bytes!("../../../etc/nebo.yaml")
  |     +-- shellexpand::env()           Expand ${VAR:-default} patterns
  |     +-- serde_yaml::from_str()       Parse YAML into Config struct
  |     +-- apply_defaults()             Fill empty fields + env var overrides
  |            |
  |            +-- NEBOAI_API_URL      Override neboai.api_url
  |            +-- NEBOAI_JANUS_URL    Override neboai.janus_url
  |            +-- NEBOAI_COMMS_URL    Override neboai.comms_url
  |
  |  3. load_settings()                  Load/create ~/.nebo/settings.json
  |     |
  |     +-- ensure_data_dir()            Create ~/.nebo/ if missing
  |     +-- Read settings.json           Parse JSON, strip BOM, trim secret
  |     +-- generate_secret()            If file missing/empty, create new
  |     +-- save_settings()              Persist with 0o600 permissions
  |
  |  4. Merge settings into config
  |     cfg.auth.access_secret  = settings.access_secret
  |     cfg.auth.access_expire  = settings.access_expire
  |     cfg.auth.refresh_token_expire = settings.refresh_token_expire
  |
  |  5. ensure_data_dir()                Create ~/.nebo/ + data/ subdirs
  |
  |  6. server::run(cfg, quiet)          Pass fully-merged Config to server
  |     |
  |     +-- ModelsConfig::load()         Load models.yaml (data_dir or embedded)
  |     +-- detect_all_clis()            Scan PATH for claude/codex/gemini
  |     +-- Build AppState { config, models_config, cli_statuses, ... }
  |
  v
RUNNING SERVER (Config available via AppState in all handlers)
```

---

## 3. Config File Hierarchy and Merge Strategy

Configuration is assembled from multiple sources. The merge order (later
overrides earlier):

```
+-------------------------------------------------------------------+
| Layer 1: Compiled defaults (Default impl on each struct)          |
|   Hardcoded constants from types::constants                       |
+---------------------------+---------------------------------------+
                            |
+---------------------------v---------------------------------------+
| Layer 2: Embedded YAML (etc/nebo.yaml, included at compile time)  |
|   Parsed via serde_yaml, ${VAR:-default} expansion via shellexpand|
+---------------------------+---------------------------------------+
                            |
+---------------------------v---------------------------------------+
| Layer 3: .env file (loaded by dotenvy before config parsing)      |
|   Makes env vars available for ${VAR} expansion in YAML           |
+---------------------------+---------------------------------------+
                            |
+---------------------------v---------------------------------------+
| Layer 4: Environment variable overrides (in apply_defaults)       |
|   NEBOAI_API_URL, NEBOAI_JANUS_URL, NEBOAI_COMMS_URL       |
|   NEBO_HOME (checked in defaults::data_dir(); NEBO_DATA_DIR=deprecated)|
+---------------------------+---------------------------------------+
                            |
+---------------------------v---------------------------------------+
| Layer 5: settings.json runtime merge (manual field assignment)    |
|   access_secret, access_expire, refresh_token_expire              |
+-------------------------------------------------------------------+
```

Key design decisions:
- There is no generic merge framework (e.g., no figment). Merging is explicit
  field-by-field assignment in the entry point (`main.rs`).
- The `#[serde(default)]` attribute on all config structs means any missing YAML
  field falls back to the Rust `Default` impl, not an error.
- `shellexpand::env()` handles `${VAR}` and `${VAR:-fallback}` syntax, enabling
  the YAML to reference environment variables with defaults.
- Settings overlay only touches three auth fields. Everything else in Config
  comes from YAML + env vars.

---

## 4. The `nebo.yaml` Embedded Config

**File:** `etc/nebo.yaml`
**Included at:** `crates/config/src/config.rs` line 347 via `include_bytes!`

This file is baked into the binary at compile time. It defines the application's
baseline configuration. Changes require recompilation.

### Complete Structure

```yaml
Name: nebo                          # Application name
Host: 127.0.0.1                    # Bind address
Port: 27895                        # HTTP server port
Timeout: 60000                     # API timeout in milliseconds

App:
  BaseURL: ${APP_BASE_URL:-http://localhost:27895}   # Shell-expandable
  Domain: ${APP_DOMAIN:-localhost}                    # Shell-expandable

Log:
  Mode: console                    # Logging mode
  Encoding: plain                  # Log encoding format
  Level: info                      # Log level: debug, info, error, severe
  Stat: false                      # Stat logs toggle

Auth:
  AccessSecret: placeholder-replaced-at-runtime   # Overwritten by settings.json
  AccessExpire: 31536000           # 1 year in seconds
  RefreshTokenExpire: 31536000     # 1 year in seconds

Database: {}                       # Uses default: ~/.nebo/data/nebo.db

NeboAI:
  Enabled: true
  ApiURL: https://api.neboai.com
  JanusURL: https://janus.neboai.com
  CommsURL: wss://comms.neboai.com/ws
```

### Fields NOT in nebo.yaml (use Defaults only)

These sections exist in the Config struct but are NOT present in the shipped
YAML. They initialize entirely from their `Default` impls:

- `Security` -- CSRF, rate limiting, headers, body size limits
- `Email` -- SMTP configuration (unused in local desktop mode)
- `OAuth` -- Google/GitHub OAuth (unused in local desktop mode)
- `Features` -- Feature flags (notifications, OAuth)
- `AppOAuth` -- Per-app OAuth provider configs (empty HashMap)
- `BrowserExtensionId` -- Optional Chrome extension ID for dev

---

## 5. The `settings.json` User Config

**File:** `~/.nebo/settings.json`
**Code:** `crates/config/src/settings.rs`

Auto-generated on first run. Contains secrets that cannot be embedded in the
binary. Unlike `nebo.yaml`, this file is read/write at runtime.

### Structure

```json
{
  "accessSecret": "a1b2c3d4...64-hex-chars...",
  "accessExpire": 31536000,
  "refreshTokenExpire": 31536000
}
```

### Fields

| Field | Type | Default | Description |
|---|---|---|---|
| `accessSecret` | string | Generated 32-byte hex | HMAC secret for JWT signing |
| `accessExpire` | i64 | 31,536,000 (1 year) | Access token TTL in seconds |
| `refreshTokenExpire` | i64 | 31,536,000 (1 year) | Refresh token TTL in seconds |

### Loading Behavior (`load_settings()`)

```
1. ensure_data_dir()                    Create ~/.nebo/ if missing
2. Check if settings.json exists
   |
   +-- YES: Read file
   |   |
   |   +-- Strip UTF-8 BOM (Windows compat)
   |   +-- Parse JSON
   |   +-- Trim whitespace from access_secret
   |   +-- If access_secret empty: generate + save
   |   +-- Return parsed Settings
   |
   +-- NO (or parse failure):
       |
       +-- Generate new 32-byte hex secret
       +-- Create Settings with defaults + new secret
       +-- save_settings() to disk
       +-- Return new Settings
```

### Saving Behavior (`save_settings()`)

- Serializes to pretty JSON via `serde_json::to_string_pretty`
- Writes atomically to `~/.nebo/settings.json`
- On Unix: sets file permissions to `0o600` (owner read/write only)

### Secret Generation (`generate_secret()`)

```rust
fn generate_secret() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill(&mut bytes);
    hex::encode(bytes)  // 64 hex characters
}
```

Uses `rand::thread_rng()` (OS-seeded CSPRNG) to generate 256 bits of entropy,
hex-encoded to a 64-character string.

---

## 6. Models Configuration (`models.yaml`)

**Embedded file:** `crates/config/src/models.yaml`
**User override:** `~/.nebo/models.yaml`
**Code:** `crates/config/src/models.rs`

### Loading Strategy (`ModelsConfig::load()`)

```
1. Try ~/.nebo/models.yaml (user override)
   |
   +-- Found + parseable:
   |   +-- Merge missing cli_providers from embedded defaults
   |   +-- Merge missing "janus" provider from embedded defaults
   |   +-- Migrate legacy janus model IDs (strip "janus/" or "neboai/" prefix)
   |   +-- Migrate "janus" -> "nebo-1" model ID
   |   +-- Return user config
   |
   +-- Not found or unparseable:
       +-- Parse embedded YAML string (include_str!)
       +-- Return embedded config (or empty fallback)
```

### Top-Level Structure

```yaml
version: "1.0"

credentials:                    # API key references (env vars)
  anthropic:
    api_key: ${ANTHROPIC_API_KEY}
  openai:
    api_key: ${OPENAI_API_KEY}
  google:
    api_key: ${GOOGLE_API_KEY}
  deepseek:
    api_key: ${DEEPSEEK_API_KEY}

defaults:                       # Primary model + fallback chain
  primary: janus/nebo-1
  fallbacks:
    - anthropic/claude-sonnet-4-6
    - anthropic/claude-haiku-4-5-20251001

task_routing:                   # Task-to-model mapping
  vision: janus/nebo-1
  audio: janus/nebo-1
  reasoning: janus/nebo-1
  code: janus/nebo-1
  general: janus/nebo-1
  fallbacks:
    vision: [anthropic/claude-sonnet-4-6, openai/gpt-5.4]
    reasoning: [anthropic/claude-opus-4-6, openai/gpt-5.4]
    code: [anthropic/claude-sonnet-4-6, openai/gpt-5.4]
    general: [anthropic/claude-haiku-4-5-20251001, openai/gpt-5.4-nano]

providers:                      # Available models per provider
  anthropic: [...]
  openai: [...]
  google: [...]
  deepseek: [...]
  janus: [...]

cli_providers:                  # CLI tool providers (no API key needed)
  - id: claude-code
    command: claude
    ...
```

### Model Definition Fields

| Field | Type | Description |
|---|---|---|
| `id` | String | Model identifier (e.g., `claude-sonnet-4-6`) |
| `displayName` | String | Human-readable name |
| `contextWindow` | i64 | Max tokens in context |
| `pricing` | Option<ModelPricing> | Cost per million tokens (input/output/cached) |
| `capabilities` | Vec<String> | Feature flags: vision, tools, streaming, code, reasoning, thinking |
| `kind` | Vec<String> | User-assigned categories |
| `preferred` | bool | User preference flag |
| `active` | Option<bool> | Whether model is enabled (default: true) |

### Model Routing Methods

```
ModelsConfig
  +-- default_model_for_provider(provider) -> Option<String>
  |     Check defaults.primary, then fallbacks, then first in provider list
  |
  +-- model_for_task(task) -> Option<String>
  |     Map task name (vision/audio/reasoning/code/general) to model ID
  |
  +-- sidecar_model() -> Option<String>
  |     Return first default fallback (cheapest model for background tasks)
  |
  +-- update_model(provider, model_id, update) -> Result
  |     Update active/kind/preferred and save to disk
  |
  +-- set_cli_provider_active(cli_id, active) -> Result
  |     Toggle CLI provider and save to disk
  |
  +-- save() -> Result
        Serialize to ~/.nebo/models.yaml
```

### CLI Provider Fields

| Field | Type | Description |
|---|---|---|
| `id` | String | Provider ID (e.g., `claude-code`) |
| `displayName` | String | Display name |
| `command` | String | CLI command name (e.g., `claude`) |
| `installHint` | String | Install instructions |
| `models` | Vec<String> | Available model IDs |
| `defaultModel` | String | Default model selection |
| `active` | Option<bool> | Enabled state (default: false) |

---

## 7. Environment Variable Overrides

### Complete List

| Variable | Where Checked | Purpose | Default |
|---|---|---|---|
| `NEBOAI_API_URL` | `Config::apply_defaults()` | NeboAI API server URL | `https://api.neboai.com` |
| `NEBOAI_JANUS_URL` | `Config::apply_defaults()` | Janus AI gateway URL | `https://janus.neboai.com` |
| `NEBOAI_COMMS_URL` | `Config::apply_defaults()` | NeboAI WebSocket URL | `wss://comms.neboai.com/ws` |
| `NEBO_HOME` | `defaults::data_dir()` | Override the Nebo root dir (`NEBO_DATA_DIR` = deprecated alias, one release) | `~/.nebo/` |
| `APP_BASE_URL` | `nebo.yaml` shell expansion | App base URL | `http://localhost:27895` |
| `APP_DOMAIN` | `nebo.yaml` shell expansion | App domain | `localhost` |
| `ANTHROPIC_API_KEY` | `models.yaml` credentials | Anthropic API key | (none) |
| `OPENAI_API_KEY` | `models.yaml` credentials | OpenAI API key | (none) |
| `GOOGLE_API_KEY` | `models.yaml` credentials | Google AI API key | (none) |
| `DEEPSEEK_API_KEY` | `models.yaml` credentials | DeepSeek API key | (none) |
| `HOME` / `USERPROFILE` | `defaults::data_dir()`, `cli_detect.rs` | Home directory | (OS-provided) |
| `PATH` | `cli_detect.rs` | CLI tool lookup | (OS-provided) |
| `APPDATA` | `cli_detect.rs` (Windows) | Windows AppData | (OS-provided) |
| `LOCALAPPDATA` | `cli_detect.rs` (Windows) | Windows LocalAppData | (OS-provided) |

### How Shell Expansion Works

The `nebo.yaml` file supports `${VAR:-default}` syntax via the `shellexpand`
crate. Before YAML parsing, the entire file content is passed through
`shellexpand::env()`:

```rust
let text = String::from_utf8_lossy(data);
let expanded = shellexpand::env(&text)?;
let config: Config = serde_yaml::from_str(&expanded)?;
```

This means any `${VAR}` reference in the YAML is replaced with the environment
variable value. If `VAR` is unset, `${VAR:-fallback}` uses the fallback.

### .env File Support

The CLI entry point calls `dotenvy::dotenv().ok()` before loading config. This
reads a `.env` file from the current working directory (if present) and injects
its key-value pairs into the process environment. Those variables are then
available for shell expansion in `nebo.yaml`.

The Tauri desktop entry point does NOT call dotenvy.

---

## 8. Data Directory System

**Code:** `crates/config/src/defaults.rs`

### Resolution Order

```
data_dir() resolution:
  1. $NEBO_HOME environment variable (if set; else deprecated $NEBO_DATA_DIR)
  2. ~/.nebo/ (HOME via dirs::home_dir())
```

### Legacy Data Directories (pre-v5)

```
legacy_data_dir() resolution:
  macOS:   ~/Library/Application Support/Nebo/
  Windows: %AppData%\Nebo\
  Linux:   ~/.config/nebo/
```

Legacy directories are auto-migrated to `~/.nebo/` on first startup (see
section 20).

### Directory Structure

```
~/.nebo/                            Root data directory
  +-- settings.json                 Local auth secrets (0o600)
  +-- bot_id                        Bot identity UUID (0o400)
  +-- models.yaml                   User model config (optional override)
  +-- .setup-complete               Setup completion marker (Unix timestamp)
  +-- .migrated-v2                  Artifact layout migration marker
  +-- .migrated-datadir-v5          Data dir migration marker
  +-- data/
  |     +-- nebo.db                 SQLite database (WAL mode)
  +-- files/
  |     +-- large_inputs/           Offloaded large tool inputs
  +-- nebo/                         Marketplace (sealed) artifacts
  |     +-- skills/                 Downloaded skill .napp files
  |     +-- agents/                 Downloaded agent .napp files
  |     +-- plugins/                Shared plugin binaries
  +-- user/                         User-created (loose) artifacts
  |     +-- skills/                 Custom skills
  |     +-- agents/                 Custom agents
  |     +-- plugins/                User plugin binaries
  +-- browser/                      Browser data (CDP profiles)
  +-- logs/                         Crash logs (Tauri only)
```

### Directory Helpers

| Function | Returns | Description |
|---|---|---|
| `data_dir()` | `Result<PathBuf>` | Root dir (`~/.nebo/` or `$NEBO_HOME`) |
| `ensure_data_dir()` | `Result<PathBuf>` | Creates root + data/ subdirs |
| `nebo_dir()` | `Result<PathBuf>` | `~/.nebo/nebo/` marketplace namespace |
| `user_dir()` | `Result<PathBuf>` | `~/.nebo/user/` user namespace |
| `ensure_artifact_dirs()` | `Result<()>` | Creates all artifact subdirectories |
| `artifact_napp_path(type, name, ver)` | `Result<PathBuf>` | Sealed .napp file path |
| `user_artifact_path(type, name)` | `Result<PathBuf>` | User artifact dir path |
| `bundled_napps_dir()` | `Option<PathBuf>` | App bundle resources (platform-specific) |

---

## 9. Key Structs and Fields

### `Config` (top-level, from nebo.yaml)

```rust
pub struct Config {
    pub name: String,              // "nebo"
    pub host: String,              // "127.0.0.1"
    pub port: u16,                 // 27895
    pub timeout: u64,              // 60000 (ms)
    pub app: AppConfig,            // Base URL, domain, production mode
    pub auth: AuthConfig,          // JWT secret, token expiry
    pub database: DatabaseConfig,  // SQLite path
    pub security: SecurityConfig,  // CSRF, rate limiting, headers
    pub email: EmailConfig,        // SMTP settings
    pub oauth: OAuthConfig,        // Google/GitHub OAuth
    pub features: FeaturesConfig,  // Feature toggles
    pub neboai: NeboAIConfig,  // NeboAI service URLs
    pub app_oauth: HashMap<String, AppOAuthProviderConfig>,  // Per-app OAuth
    pub log: LogConfig,            // Logging config
    pub browser_extension_id: Option<String>,  // Dev Chrome extension ID
}
```

### `AppConfig`

| Field | YAML Key | Type | Default |
|---|---|---|---|
| `base_url` | `BaseURL` | String | `http://localhost:27895` |
| `domain` | `Domain` | String | `localhost` |
| `production_mode` | `ProductionMode` | String | `""` (false) |
| `admin_email` | `AdminEmail` | String | `""` |

### `AuthConfig`

| Field | YAML Key | Type | Default |
|---|---|---|---|
| `access_secret` | `AccessSecret` | String | `"placeholder-replaced-at-runtime"` |
| `access_expire` | `AccessExpire` | i64 | 31,536,000 (1 year) |
| `refresh_token_expire` | `RefreshTokenExpire` | i64 | 31,536,000 (1 year) |

### `DatabaseConfig`

| Field | YAML Key | Type | Default |
|---|---|---|---|
| `sqlite_path` | `SQLitePath` | String | `~/.nebo/data/nebo.db` |

### `SecurityConfig`

| Field | YAML Key | Type | Default |
|---|---|---|---|
| `csrf_enabled` | `CSRFEnabled` | String | `"true"` |
| `csrf_secret` | `CSRFSecret` | String | `""` |
| `csrf_token_expiry` | `CSRFTokenExpiry` | i64 | 43,200 (12 hours) |
| `csrf_secure_cookie` | `CSRFSecureCookie` | String | `"true"` |
| `rate_limit_enabled` | `RateLimitEnabled` | String | `"true"` |
| `rate_limit_requests` | `RateLimitRequests` | u32 | 100 |
| `rate_limit_interval` | `RateLimitInterval` | u32 | 60 (seconds) |
| `rate_limit_burst` | `RateLimitBurst` | u32 | 20 |
| `auth_rate_limit_requests` | `AuthRateLimitRequests` | u32 | 5 |
| `auth_rate_limit_interval` | `AuthRateLimitInterval` | u32 | 60 (seconds) |
| `enable_security_headers` | `EnableSecurityHeaders` | String | `"true"` |
| `content_security_policy` | `ContentSecurityPolicy` | String | `""` |
| `allowed_origins` | `AllowedOrigins` | String | `""` |
| `force_https` | `ForceHTTPS` | String | `""` (false) |
| `max_request_body_size` | `MaxRequestBodySize` | i64 | 10,485,760 (10 MB) |
| `max_url_length` | `MaxURLLength` | u32 | 2,048 |

### `NeboAIConfig`

| Field | YAML Key | Type | Default | Env Override |
|---|---|---|---|---|
| `enabled` | `Enabled` | String | `"true"` | -- |
| `api_url` | `ApiURL` | String | `https://api.neboai.com` | `NEBOAI_API_URL` |
| `janus_url` | `JanusURL` | String | `https://janus.neboai.com` | `NEBOAI_JANUS_URL` |
| `comms_url` | `CommsURL` | String | `wss://comms.neboai.com/ws` | `NEBOAI_COMMS_URL` |

### `EmailConfig`

| Field | YAML Key | Type | Default |
|---|---|---|---|
| `smtp_host` | `SMTPHost` | String | `""` |
| `smtp_port` | `SMTPPort` | u16 | 587 |
| `smtp_user` | `SMTPUser` | String | `""` |
| `smtp_pass` | `SMTPPass` | String | `""` |
| `from_address` | `FromAddress` | String | `""` |
| `from_name` | `FromName` | String | `"nebo"` |
| `reply_to` | `ReplyTo` | String | `""` |
| `base_url` | `BaseURL` | String | `http://localhost:27458` |

### `OAuthConfig`

| Field | YAML Key | Type | Default |
|---|---|---|---|
| `google_enabled` | `GoogleEnabled` | String | `""` (false) |
| `google_client_id` | `GoogleClientID` | String | `""` |
| `google_client_secret` | `GoogleClientSecret` | String | `""` |
| `github_enabled` | `GitHubEnabled` | String | `""` (false) |
| `github_client_id` | `GitHubClientID` | String | `""` |
| `github_client_secret` | `GitHubClientSecret` | String | `""` |
| `callback_base_url` | `CallbackBaseURL` | String | `""` |

### `FeaturesConfig`

| Field | YAML Key | Type | Default |
|---|---|---|---|
| `notifications_enabled` | `NotificationsEnabled` | String | `"true"` |
| `oauth_enabled` | `OAuthEnabled` | String | `""` (false) |

### `LogConfig`

| Field | YAML Key | Type | Default |
|---|---|---|---|
| `mode` | `Mode` | String | `"console"` |
| `encoding` | `Encoding` | String | `"plain"` |
| `level` | `Level` | String | `"info"` |
| `stat` | `Stat` | bool | `false` |

### `AppOAuthProviderConfig`

| Field | YAML Key | Type | Default |
|---|---|---|---|
| `client_id` | `ClientID` | String | `""` |
| `client_secret` | `ClientSecret` | String | `""` |
| `tenant_id` | `TenantID` | String | `""` |

---

## 10. Serde Deserialization Patterns

### PascalCase YAML Keys

All YAML field names use PascalCase (matching the original Go configuration
format). Rust struct fields use snake_case. The mapping is explicit via
`#[serde(rename = "...")]`:

```rust
#[serde(rename = "AccessSecret")]
pub access_secret: String,
```

### Default Fallback

Every config struct uses `#[serde(default)]` at the struct level:

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config { ... }
```

This means if the YAML is `Database: {}` (empty map), every field in
`DatabaseConfig` gets its `Default::default()` value rather than causing a
parse error.

### Boolean-as-String Pattern

Several boolean-like fields are stored as `String` rather than `bool`. This
preserves compatibility with the original Go config format and allows three
states: `"true"`, `"false"`, and `""` (empty = use default).

The `parse_bool()` helper handles this:

```rust
fn parse_bool(s: &str, default: bool) -> bool {
    let s = s.trim().to_lowercase();
    if s.is_empty() { return default; }
    matches!(s.as_str(), "true" | "1" | "yes")
}
```

Convenience methods on `Config` wrap this:

| Method | Field | Default |
|---|---|---|
| `is_production_mode()` | `app.production_mode` | false |
| `is_csrf_enabled()` | `security.csrf_enabled` | true |
| `is_rate_limit_enabled()` | `security.rate_limit_enabled` | true |
| `is_security_headers_enabled()` | `security.enable_security_headers` | true |
| `is_force_https()` | `security.force_https` | false |
| `is_google_oauth_enabled()` | `oauth.google_enabled` | false |
| `is_github_oauth_enabled()` | `oauth.github_enabled` | false |
| `is_notifications_enabled()` | `features.notifications_enabled` | true |
| `is_oauth_enabled()` | `features.oauth_enabled` | false |
| `is_neboai_enabled()` | `neboai.enabled` | true |

### Shell Expansion Before Parsing

The YAML is preprocessed by `shellexpand::env()` BEFORE serde parses it. This
means environment variable references like `${APP_BASE_URL:-http://localhost:27895}`
are expanded to their values (or defaults) in the raw text, and serde sees only
the final string values.

---

## 11. Default Values and Fallback Behavior

### Constants from `types::constants`

```rust
// crates/types/src/constants.rs
pub const DEFAULT_PORT: u16 = 27895;
pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_ACCESS_EXPIRE: i64 = 31_536_000;       // 1 year
pub const DEFAULT_REFRESH_TOKEN_EXPIRE: i64 = 31_536_000; // 1 year
pub const DEFAULT_CSRF_TOKEN_EXPIRY: i64 = 43_200;        // 12 hours
pub const DEFAULT_RATE_LIMIT_REQUESTS: u32 = 100;
pub const DEFAULT_RATE_LIMIT_INTERVAL: u32 = 60;           // seconds
pub const DEFAULT_RATE_LIMIT_BURST: u32 = 20;
pub const DEFAULT_AUTH_RATE_LIMIT_REQUESTS: u32 = 5;
pub const DEFAULT_AUTH_RATE_LIMIT_INTERVAL: u32 = 60;      // seconds
pub const DEFAULT_MAX_REQUEST_BODY_SIZE: i64 = 10_485_760;  // 10 MB
pub const DEFAULT_MAX_URL_LENGTH: u32 = 2048;
pub const DEFAULT_SMTP_PORT: u16 = 587;
```

### File Name Constants

```rust
// types::constants::files
pub const SETTINGS_JSON: &str = "settings.json";
pub const BOT_ID: &str = "bot_id";
pub const SETUP_COMPLETE: &str = ".setup-complete";
pub const DATABASE: &str = "data/nebo.db";
pub const MODELS_YAML: &str = "models.yaml";
pub const CONFIG_YAML: &str = "config.yaml";
```

### Post-Parse Defaults (`apply_defaults()`)

After YAML parsing, `apply_defaults()` fills in any fields that were left
empty/zero:

```rust
fn apply_defaults(&mut self) {
    if self.host.is_empty()            { self.host = DEFAULT_HOST.into(); }
    if self.port == 0                  { self.port = DEFAULT_PORT; }
    if self.app.domain.is_empty()      { self.app.domain = "localhost".into(); }
    if self.app.base_url.is_empty()    { self.app.base_url = format!("http://localhost:{}", self.port); }
    if self.auth.refresh_token_expire == 0 { self.auth.refresh_token_expire = DEFAULT_REFRESH_TOKEN_EXPIRE; }
    if self.database.sqlite_path.is_empty() {
        // Resolve from data_dir()
        self.database.sqlite_path = data_dir().join("data").join("nebo.db");
    }
    // Env var overrides for NeboAI URLs
    if let Ok(v) = env::var("NEBOAI_API_URL")   { self.neboai.api_url = v; }
    if let Ok(v) = env::var("NEBOAI_JANUS_URL")  { self.neboai.janus_url = v; }
    if let Ok(v) = env::var("NEBOAI_COMMS_URL")  { self.neboai.comms_url = v; }
}
```

---

## 12. Cross-System Interactions

### Config -> Server (AppState)

The `Config` struct is stored directly in `AppState` and accessible in every
Axum handler via `State<AppState>`:

```rust
pub struct AppState {
    pub config: Config,                       // Full config clone
    pub models_config: Arc<ModelsConfig>,     // Model catalog (Arc for sharing)
    pub cli_statuses: Arc<AllCliStatuses>,    // CLI detection results
    // ... other fields
}
```

### Config -> Auth Service

`AuthService` receives a clone of `Config` at construction. It uses:
- `config.auth.access_secret` -- HMAC key for JWT signing (HS256)
- `config.auth.access_expire` -- JWT expiration delta
- `config.auth.refresh_token_expire` -- Refresh token lifetime

```rust
pub struct AuthService {
    store: Arc<Store>,
    config: Config,
}
```

### Config -> JWT Middleware

The access secret is extracted into a `JwtSecret` newtype for the Axum middleware:

```rust
pub struct JwtSecret(pub String);
let jwt_secret = JwtSecret(cfg.auth.access_secret.clone());
```

### Config -> Database

Database path flows from config to Store initialization:

```rust
let store = Arc::new(db::Store::new(&cfg.database.sqlite_path)?);
```

### Config -> AI Providers

`build_providers()` in `server/lib.rs` uses config for:
- `cfg.neboai.janus_url` -- Janus gateway base URL for the NeboAI provider
- `ModelsConfig::load()` -- Model catalog for default model selection per provider
- `detect_all_clis()` -- Whether CLI providers are available

### Config -> Tools

The tool registry receives `config.neboai.api_url` for NeboAI API calls:

```rust
let cfg = config::Config::default();
cfg.neboai.api_url  // Used in tool context
```

### Config -> Browser

`cfg.browser_extension_id` -- Optional local Chrome extension ID used to install
the native messaging host manifest alongside the production Web Store extension
ID.

### Config -> Workflow Manager

```rust
pub struct WorkflowManagerImpl {
    config: config::Config,  // For NeboAI API URL in workflow execution
}
```

---

## 13. CLI Detection System

**Code:** `crates/config/src/cli_detect.rs`

Detects installed CLI AI tools (Claude Code, OpenAI Codex, Gemini CLI) for use
as AI providers that do not require API keys.

### PATH Augmentation (`ensure_full_path()`)

GUI apps (Tauri, Finder, Start Menu) inherit a minimal PATH. Before detection,
the system augments PATH with common install locations:

```
macOS:
  ~/.npm-global/bin, ~/.nvm/versions/node/default/bin, ~/.local/bin,
  ~/.cargo/bin, /usr/local/bin, /opt/homebrew/bin, /opt/homebrew/sbin
  + Full PATH from /bin/zsh -l -c "echo $PATH"

Linux:
  ~/.npm-global/bin, ~/.nvm/versions/node/default/bin, ~/.local/bin,
  ~/.cargo/bin, /usr/local/bin, /snap/bin

Windows:
  %APPDATA%\npm, ~/.cargo/bin, %LOCALAPPDATA%\Programs\claude-code\bin,
  %LOCALAPPDATA%\Programs\codex\bin
```

PATH augmentation runs exactly once via `std::sync::Once`.

### Detection Results

```rust
pub struct AllCliStatuses {
    pub claude: CliStatus,
    pub codex: CliStatus,
    pub gemini: CliStatus,
}

pub struct CliStatus {
    pub installed: bool,
    pub authenticated: bool,
    pub version: String,
}
```

Each CLI is checked via `which::which(command)` for installation, then
`command --version` (with a 3-second timeout) for authentication and version.

---

## 14. Bot Identity System

**Code:** `crates/config/src/defaults.rs`

Each Nebo installation has a unique bot ID (UUID v4) stored at `~/.nebo/bot_id`.

### Functions

| Function | Description |
|---|---|
| `read_bot_id()` | Read from file, return None if missing or not 36 chars |
| `write_bot_id(id)` | Write to file with 0o400 permissions (read-only) |
| `ensure_bot_id()` | Read or generate new UUID and persist |

### Server Startup Flow

```
1. Check config::read_bot_id()
2. If None:
   a. Check database plugin_settings("neboai", "bot_id") -- Go migration
   b. If found in DB: write to file
   c. Else: generate new UUID, write to file
3. Sync bot_id back to DB for backward compatibility
```

The bot_id is used for:
- Janus gateway authentication (`X-Bot-ID` header)
- NeboAI device identification
- DB record correlation

---

## 15. Setup Lifecycle

**Code:** `crates/config/src/defaults.rs`

### Marker File

`~/.nebo/.setup-complete` -- contains a Unix timestamp of when setup was
completed.

### Functions

| Function | Description |
|---|---|
| `is_setup_complete()` | Check if marker file exists |
| `mark_setup_complete()` | Create marker with current Unix timestamp |

### Auto-Completion

During server startup, if `!is_setup_complete()` and `read_bot_id().is_some()`:
```rust
if !config::is_setup_complete().unwrap_or(false) {
    if config::read_bot_id().is_some() {
        config::mark_setup_complete()?;
    }
}
```

The presence of a bot_id implies the database has been initialized, which is
sufficient to consider setup complete.

---

## 16. Artifact Directory Layout

**Code:** `crates/config/src/defaults.rs` -- `ensure_artifact_dirs()`

Two namespaces for artifacts:

| Namespace | Path | Purpose |
|---|---|---|
| `nebo/` | `~/.nebo/nebo/` | Marketplace-installed sealed .napp packages |
| `user/` | `~/.nebo/user/` | User-created loose artifacts |

Each namespace has:
- `skills/` -- Skill definitions
- `agents/` -- Agent definitions

### Bundled Resources

Desktop builds include bundled .napp files in the app bundle:

```
macOS:   Nebo.app/Contents/Resources/bundled-napps/
Windows: <exe_dir>/resources/bundled-napps/
Linux:   <exe_dir>/../resources/bundled-napps/
```

`bundled_napps_dir()` returns `None` in dev/CLI mode (directory does not exist).

---

## 17. Configuration Hot-Reload

There is NO hot-reload for configuration. Changes require restarting the Nebo
process.

- `Config` (nebo.yaml) -- Compiled into the binary. Immutable at runtime.
- `Settings` (settings.json) -- Loaded once at startup. Can be regenerated
  but requires restart for changes to take effect.
- `ModelsConfig` (models.yaml) -- Re-loaded from disk on each access via
  `ModelsConfig::load()`. This is the closest thing to hot-reload: model
  catalog changes take effect on the next `load()` call without restart.
  Provider handlers call `ModelsConfig::load()` on each request, so model
  changes are effectively live.

---

## 18. Error Handling

### Error Types

The config system uses two `NeboError` variants:

```rust
#[error("config error: {0}")]
Config(String),

#[error("data directory error: {0}")]
DataDir(String),
```

### Error Sources

| Operation | Error Type | Cause |
|---|---|---|
| Shell expansion fails | `Config("env expansion: ...")` | Unknown env var without default |
| YAML parse fails | `Config("yaml parse: ...")` | Malformed YAML |
| Data dir not found | `DataDir("cannot determine home directory")` | No HOME env var |
| Dir creation fails | `DataDir("failed to create data directory: ...")` | Permission error |
| Settings serialization fails | `Config("failed to serialize settings: ...")` | (rare) |
| Settings dir creation fails | `Config("failed to create settings directory: ...")` | Permission error |

### Graceful Degradation

- `load_settings()`: If settings.json exists but is unparseable, it logs a
  warning and regenerates a fresh file rather than crashing.
- `ModelsConfig::load()`: If user models.yaml is unparseable, falls back to
  embedded defaults.
- `ensure_artifact_dirs()`: Failures are logged as warnings but do not prevent
  startup.
- `bundled_napps_dir()`: Returns `None` rather than erroring in dev mode.

---

## 19. Security Considerations

### Secret Storage

| Secret | Storage | Permissions | Notes |
|---|---|---|---|
| JWT access secret | `~/.nebo/settings.json` | 0o600 (Unix) | 256-bit CSPRNG, hex-encoded |
| Bot ID | `~/.nebo/bot_id` | 0o400 (Unix) | UUID v4, read-only after write |
| Auth placeholder | `nebo.yaml` | N/A (compiled in) | Always overwritten by settings.json |
| API keys | Database (encrypted) | DB file perms | NOT stored in config files |

### Key Security Properties

1. **JWT secret is never the YAML placeholder.** `load_settings()` always
   generates a real secret if one does not exist.

2. **settings.json is owner-only on Unix.** `save_settings()` sets 0o600
   permissions after every write.

3. **bot_id is read-only on Unix.** `write_bot_id()` sets 0o400 permissions
   to prevent accidental modification.

4. **API keys are NOT in config files.** They are stored in the SQLite database
   (`auth_profiles` table), encrypted at rest. The `credentials` section in
   `models.yaml` references env vars but is informational -- the actual keys
   are managed through the Settings UI and stored in the database.

5. **No secrets in nebo.yaml.** The embedded YAML contains only
   `placeholder-replaced-at-runtime` for the access secret. Real secrets
   come from `settings.json` or the database.

6. **BOM stripping.** `load_settings()` strips UTF-8 BOM (`\u{FEFF}`) to
   prevent silent parse failures on Windows-edited files.

7. **Whitespace trimming.** The access secret is trimmed after loading to
   prevent HMAC signature mismatches from trailing whitespace.

### Sandbox Environment

The `napp` crate defines an allowlist of environment variables that are NOT
passed to sandboxed plugin processes. `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`,
and `GOOGLE_API_KEY` are explicitly blocked from sandbox inheritance
(`crates/napp/src/sandbox.rs`).

---

## 20. Data Directory Migration

**Code:** `crates/server/src/migration.rs`

### v5: Platform-Specific to Unified (~/.nebo/)

```
Old locations:
  macOS:   ~/Library/Application Support/Nebo/
  Windows: %AppData%\Nebo\
  Linux:   ~/.config/nebo/

New location: ~/.nebo/

Migration logic:
  1. If $NEBO_HOME set: use that, no migration (deprecated $NEBO_DATA_DIR still honored)
  2. If ~/.nebo/.migrated-datadir-v5 exists: already done
  3. If old dir does not exist: fresh install, write marker
  4. If both old and new exist with content: skip (user set up manually)
  5. Otherwise: rename old -> new (or recursive copy on cross-device)
  6. Write .migrated-datadir-v5 marker
```

### v2: Artifact Layout Migration

Moves loose files from the flat layout to the namespaced layout:
- `skills/*.yaml` -> `user/skills/`
- `tools/*` -> `user/tools/`
- Marker: `.migrated-v2`

---

## 21. Testing Config

### Unit Tests (config crate)

```rust
#[test]
fn test_parse_bool() {
    assert!(parse_bool("true", false));
    assert!(parse_bool("1", false));
    assert!(parse_bool("yes", false));
    assert!(!parse_bool("false", true));
    assert!(!parse_bool("no", true));
    assert!(parse_bool("", true));   // empty uses default
    assert!(!parse_bool("", false));
}

#[test]
fn test_default_config() {
    let config = Config::default();
    assert_eq!(config.port, DEFAULT_PORT);
    assert_eq!(config.host, DEFAULT_HOST);
    assert!(config.is_csrf_enabled());
    assert!(config.is_rate_limit_enabled());
}
```

### Integration Test Pattern

The MVP readiness test in `crates/server/tests/mvp_readiness.rs` demonstrates
how to override config for testing:

```rust
// Set NEBO_HOME so config::data_dir() resolves to our temp dir
std::env::set_var("NEBO_HOME", &data_dir);

let mut cfg = config::Config::default();
cfg.database.sqlite_path = db_path.to_string_lossy().to_string();
cfg.auth.access_secret = uuid::Uuid::new_v4().to_string();
```

### CLI `config` Command

`nebo config` prints the loaded configuration for debugging:

```rust
Some(Commands::Config) => {
    println!("{cfg:#?}");
}
```

---

## 22. Dependencies

From `crates/config/Cargo.toml`:

| Dependency | Version | Purpose |
|---|---|---|
| `types` | workspace | `NeboError`, constants |
| `serde` | workspace | Derive Deserialize/Serialize |
| `serde_json` | workspace | settings.json parsing |
| `serde_yaml` | workspace | nebo.yaml and models.yaml parsing |
| `shellexpand` | workspace (v3) | `${VAR:-default}` expansion in YAML |
| `dirs` | workspace | Cross-platform home/config directory resolution |
| `rand` | workspace | CSPRNG for secret generation |
| `hex` | workspace | Hex encoding for secrets |
| `uuid` | workspace | UUID v4 generation for bot_id |
| `tracing` | workspace | Structured logging |
| `which` | v7 | CLI tool PATH lookup |

---

## Quick Reference: Entry Point Config Loading

### CLI (`crates/cli/src/main.rs`)

```rust
dotenvy::dotenv().ok();                           // Load .env
let mut cfg = config::Config::load_embedded()?;   // Parse nebo.yaml
let settings = config::load_settings()?;          // Load/create settings.json
cfg.auth.access_secret = settings.access_secret;  // Merge secrets
cfg.auth.access_expire = settings.access_expire;
cfg.auth.refresh_token_expire = settings.refresh_token_expire;
config::ensure_data_dir()?;                       // Create dirs
server::run(cfg, false).await?;                   // Pass to server
```

### Tauri Desktop (`src-tauri/src/main.rs`)

```rust
let mut cfg = config::Config::load_embedded().expect("failed to load config");
let settings = config::load_settings().expect("failed to load settings");
cfg.auth.access_secret = settings.access_secret;
cfg.auth.access_expire = settings.access_expire;
cfg.auth.refresh_token_expire = settings.refresh_token_expire;
config::ensure_data_dir().expect("failed to create data directory");
// Spawns server in background thread
server::run(cfg, true).await?;
```

Note: The Tauri entry point does NOT call `dotenvy::dotenv()`, so `.env` file
loading is CLI-only.
