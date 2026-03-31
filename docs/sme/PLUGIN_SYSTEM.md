# Plugin System — Rust SME Reference

> Definitive reference for the Nebo plugin primitive. Covers the plugin
> manifest format, PluginStore lifecycle (resolve, ensure, verify, GC,
> quarantine), SKILL.md integration, dependency cascade, code system,
> env var injection, NeboLoop API, concurrency model, and storage layout.

---

## Table of Contents

1. [System Overview](#1-system-overview)
2. [Skills vs Plugins — Decision Rule](#2-skills-vs-plugins--decision-rule)
3. [Architecture & Crate Placement](#3-architecture--crate-placement)
4. [Plugin Types](#4-plugin-types)
5. [PluginStore](#5-pluginstore)
6. [SKILL.md Integration](#6-skillmd-integration)
7. [Skill Loader Integration](#7-skill-loader-integration)
8. [ExecuteTool Integration](#8-executetool-integration)
9. [Code System (PLUG-XXXX-XXXX)](#9-code-system-plug-xxxx-xxxx)
10. [Dependency Cascade](#10-dependency-cascade)
11. [NeboLoop API](#11-neboloop-api)
12. [AppState Wiring](#12-appstate-wiring)
13. [Sandbox Policy](#13-sandbox-policy)
14. [Storage Layout](#14-storage-layout)
15. [Concurrency Model](#15-concurrency-model)
16. [Platform Detection](#16-platform-detection)
17. [Precedence Rule](#17-precedence-rule)
18. [WebSocket Events](#18-websocket-events)
19. [Edge Cases](#19-edge-cases)
20. [Key Files](#20-key-files)
21. [NeboLoop MCP Server — Plugin Tool](#21-neboloop-mcp-server--plugin-tool)

---

## 1. System Overview

Nebo has two ways to distribute native binaries to agents. Both follow the [Agent Skills](https://agentskills.io) open format for SKILL.md, with Nebo-specific extensions for binary distribution.

```
Skills      — markdown knowledge + optional embedded binary (agentskills.io spec)
Plugins     — shared managed binaries that multiple skills depend on
Extensions  — deep integrations (Chrome bridge, etc.)
```

**Skills can have their own binaries.** A skill uploaded via `skill(action: binary-token)` bundles a native binary that is tightly coupled to that one skill. The binary is downloaded when the skill is installed.

**Plugins are shared binaries.** A plugin is a standalone binary artifact that multiple skills can depend on. When any skill declaring `plugins: [{name: gws}]` is installed, the plugin binary is downloaded once and shared. If 3 skills depend on `gws`, only one copy exists on disk.

**Key properties (plugins):**
- **Zero-click install.** Plugin binaries download silently during skill install. No approval dialog.
- **Shared across skills.** If 3 skills depend on `gws`, only one copy is downloaded.
- **Platform-specific.** Each plugin has per-platform binaries (darwin-arm64, linux-amd64, etc.).
- **Semver range matching.** Skills declare version ranges (`>=1.2.0`, `^1.0.0`, `*`).
- **SHA256 + ED25519 verification.** Binary integrity verified on download; signature verified if signing key available.
- **Env var injection.** Scripts access the binary via `{SLUG}_BIN` environment variable (e.g., `GWS_BIN=/path/to/gws`).

---

## 2. Skills vs Plugins — Decision Rule

> **One binary, one skill → skill with binary.**
> **One binary, many skills → plugin.**

This is the only rule. If a binary is used by exactly one skill, embed it as a skill with binary. If multiple skills share the same binary, make it a plugin.

| Binary | Used by | Artifact type | Why |
|--------|---------|---------------|-----|
| `nebo-pdf` | pdf skill only | Skill with binary | 1 skill, 1 binary |
| `nebo-office` | docx, xlsx, pptx skills | Plugin | 3 skills share 1 binary |
| `gws` | gmail, calendar, drive, sheets skills | Plugin | Many skills share 1 binary |
| `ffmpeg` | video-encode, audio-extract skills | Plugin | Many skills share 1 binary |

### Skills with Binaries

Published via the `skill(...)` MCP tool. The binary is uploaded alongside the SKILL.md:

```
1. skill(action: create, name: "pdf", manifestContent: "...")
2. skill(action: binary-token, id: "<SKILL_ID>")
3. curl upload: -F "file=@./nebo-pdf" -F "platform=darwin-arm64" -F "skill=@./SKILL.md"
4. skill(action: submit, id: "<SKILL_ID>", version: "1.0.0")
```

The SKILL.md follows the [agentskills.io specification](https://agentskills.io/specification) exactly. Required frontmatter: `name`, `description`. Optional: `license`, `compatibility`, `metadata`, `allowed-tools`.

### Plugins

Published via the `plugin(...)` MCP tool. The binary is a separate artifact that skills reference:

```
1. plugin(action: create, name: "gws", category: "connectors", version: "0.22.3")
2. plugin(action: binary-token, id: "<PLUGIN_ID>")
3. curl upload: -F "file=@./gws" -F "platform=darwin-arm64" -F "skill=@./PLUGIN.md"
4. plugin(action: submit, id: "<PLUGIN_ID>", version: "0.22.3")
```

Then, separately, create skills that depend on the plugin:
```
skill(action: create, name: "gmail", manifestContent: "---\nplugins:\n  - name: gws\n    version: \">=0.22.0\"\n---\n...")
```

---

## 3. Architecture & Crate Placement

Plugin lifecycle belongs in **`crates/napp/`** — not `crates/tools/`. The napp crate already manages binary artifacts (Registry, signing, versioning, quarantine). A parallel `PluginRegistry` in tools would create competing pathways.

| Concern | Crate | File | Rationale |
|---------|-------|------|-----------|
| `PluginManifest`, `PlatformBinary` types | napp | `plugin.rs` | Binary artifact types |
| `PluginStore` (download, verify, store) | napp | `plugin.rs` | Reuses SigningKeyProvider, version resolution |
| `current_platform_key()`, `plugin_env_var()` | napp | `plugin.rs` | Platform detection for binaries |
| `PluginDependency` on `Skill` struct | tools | `skills/skill.rs` | Skill schema definition |
| `verify_dependencies()` plugin check | tools | `skills/loader.rs` | Calls into `napp::plugin` |
| Env var injection | tools | `execute_tool.rs` | Runtime integration |
| `PLUG-XXXX-XXXX` code handling | server | `codes.rs` | Code dispatch |
| `DepType::Plugin` cascade | server | `deps.rs` | Dependency resolution |
| `plugin_store` on AppState | server | `state.rs` | Shared state |
| Plugin init + loader wiring | server | `lib.rs` | Startup |
| `get_plugin()`, `download_plugin_binary()` | comm | `api.rs` | NeboLoop REST API |

---

## 4. Plugin Types

**Source:** `crates/napp/src/plugin.rs`

### PluginManifest

Stored locally at `<data_dir>/nebo/plugins/<slug>/<version>/plugin.json`. All fields use camelCase serde.

```rust
pub struct PluginManifest {
    pub id: String,                                  // NeboLoop artifact ID
    pub slug: String,                                // URL-safe slug, matches skill's plugins[].name
    pub name: String,                                // Human-readable display name
    pub version: String,                             // Semver version string
    pub description: String,                         // Brief description
    pub author: String,                              // Publisher name
    pub platforms: HashMap<String, PlatformBinary>,   // "macos-arm64" → binary info
    pub signing_key_id: String,                      // ED25519 signing key ID
    pub env_var: String,                             // Custom env var name override (default: {SLUG}_BIN)
    pub auth: Option<PluginAuth>,                    // Optional authentication configuration
}
```

### PluginAuth

Authentication configuration for plugins that require credentials (e.g., OAuth for Google Workspace). Declared in `plugin.json`, read by handlers at `crates/server/src/handlers/plugins.rs`.

```rust
pub struct PluginAuth {
    pub auth_type: String,                           // Auth type identifier (e.g., "oauth_cli")
    pub env: HashMap<String, String>,                // Env vars injected before running auth commands
    pub commands: PluginAuthCommands,                // CLI subcommands for auth lifecycle
    pub label: String,                               // Human-readable label for UI (e.g., "Google Account")
    pub description: String,                         // Description shown to user during auth step
}

pub struct PluginAuthCommands {
    pub login: String,                               // Subcommand to trigger auth (e.g., "auth login")
    pub status: Option<String>,                      // Subcommand to check auth status (exit 0 = authenticated)
    pub logout: Option<String>,                      // Subcommand to clear credentials (e.g., "auth logout")
}
```

**JSON example** (in `plugin.json`):

```json
{
  "slug": "gws",
  "name": "Google Workspace CLI",
  "auth": {
    "type": "oauth_cli",
    "label": "Google Account",
    "description": "Authenticate with your Google Workspace account.",
    "commands": {
      "login": "auth login",
      "status": "auth status",
      "logout": "auth logout"
    },
    "env": {}
  }
}
```

**Auth flow:**

1. `GET /api/v1/plugins` — returns `hasAuth: true` + `authLabel` for plugins with auth config
2. `GET /api/v1/plugins/{slug}/auth/status` — runs `<binary> <status_cmd>`, exit code 0 = authenticated
3. `POST /api/v1/plugins/{slug}/auth/login` — spawns `<binary> <login_cmd>` in background, broadcasts `plugin_auth_complete` or `plugin_auth_error` via WebSocket
4. `POST /api/v1/plugins/{slug}/auth/logout` — runs `<binary> <logout_cmd>` synchronously

### PlatformBinary

Per-platform binary entry within a PluginManifest.

```rust
pub struct PlatformBinary {
    pub binary_name: String,    // "gws" or "gws.exe"
    pub sha256: String,         // SHA256 hex hash
    pub signature: String,      // ED25519 signature (base64)
    pub size: u64,              // File size in bytes
    pub download_url: String,   // CDN URL or API path
}
```

### PluginDependency

Declared in SKILL.md frontmatter. **Source:** `crates/tools/src/skills/skill.rs`

```rust
pub struct PluginDependency {
    pub name: String,       // Plugin slug (matches PluginManifest.slug)
    pub version: String,    // Semver range, default "*"
    pub optional: bool,     // Default false — if true, skill loads without this plugin
}
```

---

## 5. PluginStore

**Source:** `crates/napp/src/plugin.rs`

Core struct managing the plugin lifecycle. Constructed once at startup, stored as `Arc<PluginStore>` on AppState.

```rust
pub struct PluginStore {
    installed_dir: PathBuf,                                         // <data_dir>/nebo/plugins/
    user_dir: PathBuf,                                              // <data_dir>/user/plugins/
    signing_key: Option<Arc<SigningKeyProvider>>,                    // ED25519 verification
    manifests: Arc<tokio::sync::RwLock<HashMap<String, PluginManifest>>>,  // Cache keyed by "slug:version"
    downloading: Arc<tokio::sync::Mutex<HashSet<String>>>,          // Concurrent download dedup
}
```

### Methods

| Method | Async | Description |
|--------|-------|-------------|
| `new(plugins_dir, signing_key)` | No | Constructor |
| `plugins_dir()` | No | Returns installed dir path (`&Path`) |
| `resolve(slug, version_range)` | No | Local-only semver resolution → `Option<PathBuf>` |
| `ensure(slug, version_range, download_fn)` | Yes | Resolve locally or download, returns binary path |
| `install_from_napp(slug, napp_data)` | Yes | Install from .napp archive (binary + plugin.json + PLUGIN.md + skills/) |
| `verify_integrity(slug, version)` | No | SHA256 check against cached manifest |
| `list_installed()` | No | All installed `(slug, Version, PathBuf)` tuples |
| `garbage_collect(referenced_slugs)` | No | Remove unreferenced plugin slug directories |
| `quarantine(slug, version, reason)` | No | Delete binary, write `.quarantined` marker |

### resolve()

Non-async, local filesystem only. Safe to call from sync contexts.

1. Scans `<plugins_dir>/<slug>/` for version directories
2. Parses each directory name as semver
3. Filters by version range (uses `semver::VersionReq::parse()`)
4. Skips quarantined versions (`.quarantined` marker file)
5. Finds binary via `find_binary_in_version_dir()` (manifest first, then executable scan)
6. Returns the highest matching version's binary path

### ensure()

Async download with dedup.

1. **Fast path:** Call `resolve()` — if found, return immediately
2. **Download dedup:** Check `downloading` mutex. If slug already being downloaded, poll `resolve()` every 1s for up to 30s
3. **Download:** Call `download_fn(slug, platform)` which returns `(PluginManifest, Vec<u8>)`
4. **Verify SHA256:** Compute hash of downloaded bytes, compare to `platform_binary.sha256`
5. **Verify ED25519:** If `signing_key` is Some, decode signature from base64, verify against binary data
6. **Store:** Write binary to `<plugins_dir>/<slug>/<version>/<binary_name>`, chmod 755
7. **Write manifest:** Serialize to `plugin.json` in version directory
8. **Cache:** Insert manifest into in-memory `manifests` map
9. **Release:** Remove slug from `downloading` set

### install_from_napp()

Install a plugin from a sealed `.napp` archive. Used by `handle_plugin_code()` when `CodeRedeemResponse.download_url` ends in `.napp`.

```
1. Write napp_data to temp file
2. extract_napp() → temp directory
3. Read plugin.json from extracted dir → PluginManifest
4. Look up platform binary entry for current_platform_key()
5. Verify SHA256 of binary against manifest
6. Verify ED25519 signature if signing_key available
7. copy_dir_recursive() → <installed_dir>/<slug>/<version>/
8. Cache manifest in memory
9. Return binary path
```

The extracted .napp contains: `manifest.json`, `plugin.json`, `PLUGIN.md`, the native binary, and optionally `skills/{name}/SKILL.md` entries. After extraction, all files land in the version directory — the skill loader picks up embedded skills from there.

### find_binary_in_version_dir()

Two-step binary discovery:

1. Read `plugin.json` → look up current platform → check if `binary_name` file exists
2. Fallback: scan directory for first executable file (Unix: mode & 0o111; Windows: .exe/.bat/.cmd)

---

## 6. SKILL.md Integration

Nebo skills follow the [Agent Skills](https://agentskills.io) open format. The SKILL.md file must contain YAML frontmatter followed by Markdown content per the [specification](https://agentskills.io/specification).

### Standard Fields (agentskills.io spec)

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Lowercase letters, numbers, hyphens. Max 64 chars. Must match parent directory name. |
| `description` | Yes | What the skill does and when to use it. Max 1024 chars. |
| `license` | No | License name or reference to bundled license file. |
| `compatibility` | No | Environment requirements (system packages, network access, etc.). |
| `metadata` | No | Arbitrary key-value mapping. |
| `allowed-tools` | No | Space-delimited list of pre-approved tools. (Experimental) |

### Nebo Extension: `plugins` Field

Nebo extends the frontmatter with a `plugins` array to declare plugin dependencies:

```yaml
---
name: gmail
description: Send and read Gmail messages. Use when the user mentions Gmail, email, or inbox.
license: MIT
plugins:
  - name: gws
    version: ">=0.22.0"
  - name: ffmpeg
    version: ">=5.0.0"
    optional: true
---
```

- `name` matches the plugin's `slug` in NeboLoop
- `version` is a semver range string (default `"*"`)
- `optional: true` means the skill loads even if this plugin isn't installed

The `plugins` field is parsed into `Vec<PluginDependency>` on the `Skill` struct.

### Progressive Disclosure

Following the agentskills.io model, skills use progressive disclosure:

1. **Discovery** (~100 tokens): `name` and `description` loaded at startup for all skills
2. **Activation** (< 5000 tokens recommended): Full SKILL.md body loaded when skill is activated
3. **Resources** (as needed): Files in `scripts/`, `references/`, `assets/` loaded on demand

---

## 7. Skill Loader Integration

**Source:** `crates/tools/src/skills/loader.rs`

The `Loader` struct has an optional `plugin_store: Option<Arc<napp::plugin::PluginStore>>` field, set via the builder method `with_plugin_store()`.

### Plugin Directory Scanning (Tier 2.5)

During `load_all()`, after installed skills (tier 2) and before user skills (tier 3), the loader scans plugin directories for embedded skills:

```
1. Bundled (bundled_dir)
2. Installed (installed_dir) — marketplace .napp skills
2.5. Plugin-embedded (plugin_store.plugins_dir()) — skills inside plugin .napp bundles
3. User (user_dir)
4. Legacy YAML (user_dir)
```

The plugin scan uses `load_skills_from_nested_dir()` with `walk_for_marker("SKILL.md")`, which recursively finds paths like `plugins/gws/0.22.3/skills/gws-gmail/SKILL.md`. All discovered skills are force-enabled.

### Plugin Dependency Verification

After loading all tiers, `verify_dependencies()` checks plugin dependencies:

```
For each loaded skill:
  For each plugin in skill.plugins:
    If plugin.optional → skip
    If plugin_store.resolve(plugin.name, plugin.version) → None:
      Drop skill with warning: "skill skipped: missing plugin"
```

Skills with missing **required** plugin dependencies are removed from the loaded set. Optional plugins are silently skipped.

### Hot-Reload Watching

The `watch()` method watches the plugin installed dir alongside `user_dir` and `installed_dir`. When a plugin is installed or removed, the watcher triggers a full `load_all()` reload, picking up or removing plugin-embedded skills.

The hot-reload watcher clones the `plugin_store` Arc and passes it to the reload closure, ensuring plugin checks happen on every reload cycle.

---

## 8. ExecuteTool Integration

**Source:** `crates/tools/src/execute_tool.rs`

The `ExecuteTool` struct has `plugin_store: Option<Arc<napp::plugin::PluginStore>>`, set via `with_plugin_store()`.

**Env var injection** happens after secret injection (line ~303), before the script subprocess is spawned:

```rust
if let Some(ref plugin_store) = self.plugin_store {
    for p in &skill.plugins {
        if let Some(binary_path) = plugin_store.resolve(&p.name, &p.version) {
            let env_name = napp::plugin::plugin_env_var(&p.name);
            cmd.env(&env_name, binary_path.to_string_lossy().as_ref());
        }
    }
}
```

This means:
- Plugin `gws` version `>=1.2.0` resolves to e.g., `/data/nebo/plugins/gws/1.2.0/gws`
- Environment variable `GWS_BIN` is set to that path
- The skill's script can use `$GWS_BIN` to invoke the binary

---

## 9. Code System (PLUG-XXXX-XXXX)

**Source:** `crates/server/src/codes.rs`

Plugins have their own install code prefix, following the same Crockford Base32 pattern as other artifacts:

| Prefix | Artifact |
|--------|----------|
| `NEBO` | Link bot to NeboLoop account |
| `SKIL` | Install a skill |
| `WORK` | Install a workflow |
| `AGNT` | Install an agent |
| `LOOP` | Join bot to a Loop |
| `PLUG` | Install a plugin |

`detect_code()` checks for the `PLUG-` prefix and returns `CodeType::Plugin`.

`handle_plugin_code()` flow:
1. Build NeboLoop API client
2. Redeem code via `api.install_skill(code)` (plugins use the same install endpoint)
3. Detect platform via `napp::plugin::current_platform_key()`
4. Broadcast `plugin_installing` WebSocket event
5. Check if `CodeRedeemResponse.download_url` ends in `.napp`:
   - **Yes (.napp path):** Download .napp via `api.download_napp()` → `plugin_store.install_from_napp()` — extracts binary + plugin.json + PLUGIN.md + skills/ in one shot
   - **No (binary-only fallback):** Call `plugin_store.ensure()` with a download callback that queries `api.get_plugin()` and `api.download_plugin_binary()`
6. On success: broadcast `plugin_installed` event, reload skill loader (picks up embedded skills)
7. On failure: broadcast `plugin_error` event

Both the WebSocket handler and the REST `POST /api/v1/codes/redeem` handler dispatch to `handle_plugin_code()`.

---

## 10. Dependency Cascade

**Source:** `crates/server/src/deps.rs`

The dependency cascade resolver now includes `DepType::Plugin` alongside `DepType::Skill` and `DepType::Workflow`.

### extract_skill_deps()

Extracts plugin dependencies from a skill's `plugins` field (non-optional only):

```rust
for plugin in &skill.plugins {
    if !plugin.optional {
        deps.push(DepRef {
            dep_type: DepType::Plugin,
            reference: plugin.name.clone(),
        });
    }
}
```

### is_installed()

For plugins: `state.plugin_store.resolve(&dep.reference, "*").is_some()`

### install_dep()

For plugins: calls `install_plugin()` which:
1. Extracts the simple slug name from the reference
2. Builds a NeboLoop API client
3. Calls `plugin_store.ensure()` with a download callback
4. Returns empty child deps (plugins are leaf nodes — they don't have further dependencies)

---

## 11. NeboLoop API

**Source:** `crates/comm/src/api.rs`

Two new methods on `NeboLoopApi`:

### get_plugin(slug, platform)

```
GET /api/v1/plugins/{slug}?platform={platform}
```

Returns `napp::plugin::PluginManifest`. The server filters platform binaries based on the `platform` query parameter.

### download_plugin_binary(url)

Downloads the binary bytes from the given URL. Handles both absolute URLs (CDN) and relative URLs (API paths resolved against `api_server`).

Returns `Vec<u8>`.

---

## 12. AppState Wiring

**Source:** `crates/server/src/state.rs`, `crates/server/src/lib.rs`

`AppState` has a `plugin_store: Arc<napp::plugin::PluginStore>` field.

Initialization in `lib.rs`:

```rust
let plugins_dir = data_dir.join("nebo").join("plugins");
let _ = std::fs::create_dir_all(&plugins_dir);
let plugin_store = Arc::new(napp::plugin::PluginStore::new(plugins_dir, None));
```

The plugin store is wired to the skill loader and included in AppState construction:

```rust
let skill_loader = Loader::new(bundled_dir, installed_dir, user_dir)
    .with_plugin_store(plugin_store.clone());
```

**Note:** `signing_key` is currently `None`. When NeboLoop's ED25519 public key infrastructure is fully deployed, this will be wired to `Arc<SigningKeyProvider>`.

---

## 13. Sandbox Policy

**Source:** `crates/tools/src/sandbox_policy.rs`

No sandbox policy changes were needed. The sandbox uses a **deny-list** model for filesystem reads (not an allow-list). Since `<data_dir>/nebo/plugins/` is not on any deny list, plugin binaries are readable by default.

Plugin binaries are executed as subprocesses by the skill's script (e.g., `$GWS_BIN --list-emails`), not by the sandbox directly. The script subprocess inherits the sandbox profile, which allows read access to the plugin binary path.

---

## 14. Storage Layout

```
<data_dir>/
  nebo/
    skills/           # Marketplace skills (existing)
    plugins/          # Plugin binaries
      gws/
        0.22.3/
          manifest.json   # Package identity + metadata
          plugin.json     # Cached PluginManifest (JSON, camelCase)
          PLUGIN.md       # Plugin documentation
          gws             # Native binary (chmod 755)
          skills/         # Embedded skills (from .napp bundle)
            gws-gmail/
              SKILL.md
            gws-calendar/
              SKILL.md
            gws-drive/
              SKILL.md
      nebo-office/
        1.0.0/
          manifest.json
          plugin.json
          PLUGIN.md
          nebo-office
          skills/
            xlsx/
              SKILL.md
            docx/
              SKILL.md
```

- Directory per slug, subdirectory per version
- `plugin.json` = serialized `PluginManifest` with full platform map
- `manifest.json` = package identity (required by .napp extraction)
- `PLUGIN.md` = plugin documentation/instructions
- `skills/` = embedded SKILL.md files, discovered by skill loader via `walk_for_marker()`
- Binary = native executable, platform-specific
- `.quarantined` marker file = quarantined version (binary deleted, manifest preserved)
- Multiple versions can coexist (different skills may require different ranges)

---

## 15. Concurrency Model

| Concern | Solution |
|---------|----------|
| Manifests cache | `tokio::sync::RwLock<HashMap>` — async reads, `ensure()` holds write across `.await` |
| Concurrent downloads | `downloading: Arc<tokio::sync::Mutex<HashSet<String>>>` — check-then-insert dedup. Second caller polls `resolve()` every 1s for 30s |
| GC vs reload race | GC takes `HashSet<String>` snapshot (not `&[Skill]`), snapshot-then-release pattern |
| `resolve()` is sync | Local filesystem only — no async needed, safe to call from sync contexts |

---

## 16. Platform Detection

**Source:** `crates/napp/src/plugin.rs`

```rust
pub fn current_platform_key() -> String {
    let os = std::env::consts::OS;       // "macos", "linux", "windows"
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => other,
    };
    format!("{}-{}", os, arch)
}
```

Valid platform keys (matching NeboLoop conventions):
- `macos-arm64` (Apple Silicon)
- `macos-amd64` (Intel Mac)
- `linux-arm64`
- `linux-amd64`
- `windows-arm64`
- `windows-amd64`

### plugin_env_var()

```rust
pub fn plugin_env_var(slug: &str) -> String {
    format!("{}_BIN", slug.to_uppercase().replace('-', "_"))
}
```

| Slug | Env Var |
|------|---------|
| `gws` | `GWS_BIN` |
| `my-tool` | `MY_TOOL_BIN` |
| `ffmpeg` | `FFMPEG_BIN` |

If `PluginManifest.env_var` is non-empty, the custom name is used instead. (Not yet wired in ExecuteTool — uses default convention.)

---

## 17. Precedence Rule

When a skill has BOTH an embedded binary (`RuntimeKind::Binary` from `.napp`) AND a `plugins:` dependency for the same tool:

- **Embedded binary wins.** The `execute_tool.rs` binary detection (lines ~416-443) runs BEFORE plugin env var injection.
- Plugin env vars are injected for scripts to reference, not for the execute tool's own binary detection.
- This is the expected behavior: embedded = bundled with skill, plugin = available for scripts.

---

## 18. WebSocket Events

Events broadcast during plugin operations (camelCase per convention):

| Event | Payload | When |
|-------|---------|------|
| `plugin_installing` | `{ plugin, platform }` | Before download starts |
| `plugin_installed` | `{ plugin }` | After successful install |
| `plugin_error` | `{ plugin, error }` | On download/verify failure |

Future events (not yet implemented):
- `plugin_progress` — download progress (`{ plugin, downloaded, total }`)
- `plugin_updated` — version upgrade (`{ plugin, fromVersion, toVersion }`)
- `plugin_quarantined` — revocation (`{ plugin, reason }`)

---

## 19. Edge Cases

- **Offline:** `resolve()` is local-only. Works without network after first download.
- **Platform unavailable:** `NappError::PluginPlatformUnavailable` → "This skill isn't available for your platform yet."
- **Corruption:** SHA256 check on download. `verify_integrity()` for periodic checks.
- **Update while running:** New version in separate directory; old binary handle stays open (Unix).
- **Revocation:** Extend existing `RevocationChecker` to include plugin IDs. `quarantine()` removes binary, writes marker.
- **GC:** `garbage_collect()` takes a snapshot of referenced slugs, then removes unreferenced slug directories. Deferred, not eager.
- **Concurrent download:** `downloading` mutex deduplicates — second caller polls for first to complete (30s timeout).
- **Invalid version range:** `semver::VersionReq::parse()` fails → `resolve()` returns None.
- **Empty version range / `*`:** Matches any installed version, returns highest.

---

## 20. Key Files

| File | Lines | What |
|------|-------|------|
| `crates/napp/src/plugin.rs` | ~900 | Core module: types, PluginStore (ensure + install_from_napp), helpers, tests |
| `crates/napp/src/napp.rs` | — | .napp extraction: ALLOWED_FILES includes PLUGIN.md/plugin.json, `skills/` prefix support |
| `crates/napp/src/lib.rs` | — | `pub mod plugin;` + NappError variants |
| `crates/tools/src/skills/skill.rs` | — | `PluginDependency` struct, `plugins` field on `Skill` |
| `crates/tools/src/skills/loader.rs` | — | `plugin_store` field, `verify_dependencies()` plugin check |
| `crates/tools/src/execute_tool.rs` | — | `plugin_store` field, env var injection |
| `crates/server/src/state.rs` | — | `plugin_store: Arc<PluginStore>` on AppState |
| `crates/server/src/lib.rs` | — | Plugin store init, loader wiring |
| `crates/server/src/codes.rs` | — | `CodeType::Plugin`, `PLUG-` detection, `handle_plugin_code()` |
| `crates/server/src/deps.rs` | — | `DepType::Plugin`, `install_plugin()`, `extract_skill_deps()` |
| `crates/comm/src/api.rs` | — | `get_plugin()`, `download_plugin_binary()` |

---

## 21. NeboLoop MCP Server — Plugin Tool

Plugins are a **first-class artifact type** on NeboLoop, alongside skills and agents. Each has its own dedicated MCP tool — plugins are never created through the skill tool.

### Three Artifact Types

| MCP Tool | DB Type | Code Prefix | Manifest |
|----------|---------|-------------|----------|
| `skill(...)` | `skill` | `SKIL` | SKILL.md |
| `plugin(...)` | `plugin` | `PLUG` | PLUGIN.md |
| `agent(...)` | `agent` | `AGNT` | AGENT.md |

### Plugin Tool Actions

**Source:** `neboloop/internal/mcp/tools/plugin.go`

| Action | Description |
|--------|-------------|
| `plugin(action: list)` | List all your plugins |
| `plugin(action: get, id: "...")` | Get plugin details |
| `plugin(action: create, name: "...")` | Create a new plugin artifact |
| `plugin(action: update, id: "...")` | Update plugin metadata |
| `plugin(action: delete, id: "...")` | Delete a plugin |
| `plugin(action: submit, id: "...", version: "...")` | Submit for marketplace review (requires developer account) |
| `plugin(action: list-binaries, id: "...")` | List uploaded binaries (requires developer account) |
| `plugin(action: binary-token, id: "...")` | Generate upload token + curl command (requires developer account) |
| `plugin(action: delete-binary, id: "...")` | Delete a binary by ID (requires developer account) |

### Publishing a Plugin — Step by Step

```
1. Select developer account:
   developer(resource: account, action: select, id: "<dev-account-id>")

2. Create the plugin artifact:
   plugin(action: create, name: "gws", category: "connectors", version: "1.0.0")

3. Get an upload token (returns a curl command):
   plugin(action: binary-token, id: "<PLUGIN_ID>")

4. Upload binary per platform (via curl from step 3):
   curl -X POST https://neboloop.com/api/v1/developer/apps/<PLUGIN_ID>/binaries \
     -H "Authorization: Bearer <token>" \
     -F "file=@./target/release/gws" \
     -F "platform=darwin-arm64" \
     -F "skill=@./PLUGIN.md"

   Repeat for each platform: darwin-arm64, darwin-amd64, linux-arm64, linux-amd64, etc.

5. Submit for review:
   plugin(action: submit, id: "<PLUGIN_ID>", version: "1.0.0")
```

### Server-Side Architecture

| File | What |
|------|------|
| `neboloop/internal/mcp/tools/plugin.go` | Plugin MCP tool — all 9 actions |
| `neboloop/internal/mcp/server.go` | `RegisterPluginTool()` registration |
| `neboloop/internal/mcp/tools/registry.go` | `registerPluginToolToRegistry()` |
| `neboloop/internal/marketplace/service.go` | `CodePrefix("plugin")` → `"PLUG"` |
| `neboloop/internal/db/queries/marketplace_categories.sql` | `plugin_count` in category counts |

### Code Prefix Generation

```go
func CodePrefix(artifactType string) string {
    switch artifactType {
    case "agent":
        return "AGNT"
    case "plugin":
        return "PLUG"
    default:
        return "SKIL"
    }
}
```

### Database

Plugins use the unified `artifacts` table with `type = 'plugin'`. The DB constraint allows: `type IN ('skill', 'plugin', 'agent')`.

Binary uploads go to `artifact_binaries` table with `(artifact_id, version, platform)` unique constraint. Storage key: `binaries/{plugin_id}/{version}/{platform}`.

### Marketplace Integration

- `marketplace(action: search, type: "plugin")` — filter by plugin type
- `marketplace(action: list_categories, withCounts: true)` — includes `pluginCount` per category
- Featured, popular, and recent queries all support `type: "plugin"` filter

### Access Control

Plugin access uses the same namespace-based model as skills: `canAccessPlugin()` checks that the plugin's namespace matches the developer account's namespace, or the plugin is owned by the current user.

---

## Cross-References

- **Agent Skills Spec:** https://agentskills.io/specification — the open SKILL.md format we adhere to
- **Skills SME:** `docs/sme/SKILLS_SME.md` — SKILL.md format, loader, ExecuteTool, sandbox
- **Publisher's Guide:** `docs/publishers-guide/plugins.md` — how to create and publish plugins
- **Packaging:** `docs/publishers-guide/packaging.md` — .napp archives, qualified names, install codes
- **Security SME:** `docs/sme/SECURITY.md` — signing, verification, sandbox
