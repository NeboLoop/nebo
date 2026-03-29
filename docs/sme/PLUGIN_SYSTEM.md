# Plugin System — Rust SME Reference

> Definitive reference for the Nebo plugin primitive. Covers the plugin
> manifest format, PluginStore lifecycle (resolve, ensure, verify, GC,
> quarantine), SKILL.md integration, dependency cascade, code system,
> env var injection, NeboLoop API, concurrency model, and storage layout.

---

## Table of Contents

1. [System Overview](#1-system-overview)
2. [Architecture & Crate Placement](#2-architecture--crate-placement)
3. [Plugin Types](#3-plugin-types)
4. [PluginStore](#4-pluginstore)
5. [SKILL.md Integration](#5-skillmd-integration)
6. [Skill Loader Integration](#6-skill-loader-integration)
7. [ExecuteTool Integration](#7-executetool-integration)
8. [Code System (PLUG-XXXX-XXXX)](#8-code-system-plug-xxxx-xxxx)
9. [Dependency Cascade](#9-dependency-cascade)
10. [NeboLoop API](#10-neboloop-api)
11. [AppState Wiring](#11-appstate-wiring)
12. [Sandbox Policy](#12-sandbox-policy)
13. [Storage Layout](#13-storage-layout)
14. [Concurrency Model](#14-concurrency-model)
15. [Platform Detection](#15-platform-detection)
16. [Precedence Rule](#16-precedence-rule)
17. [WebSocket Events](#17-websocket-events)
18. [Edge Cases](#18-edge-cases)
19. [Key Files](#19-key-files)

---

## 1. System Overview

A **Plugin** is a managed native binary downloaded once from NeboLoop and shared across skills. It sits alongside skills and extensions in the artifact hierarchy:

```
Skills      — pure markdown knowledge, injected into context
Plugins     — managed binaries that skills depend on (gws, ffmpeg, etc.)
Extensions  — deep integrations (Chrome bridge, etc.)
```

**Key properties:**
- **Zero-click install.** Plugin binaries download silently during skill install. No approval dialog.
- **Shared across skills.** If 3 skills depend on `gws`, only one copy is downloaded.
- **Platform-specific.** Each plugin has per-platform binaries (darwin-arm64, linux-amd64, etc.).
- **Semver range matching.** Skills declare version ranges (`>=1.2.0`, `^1.0.0`, `*`).
- **SHA256 + ED25519 verification.** Binary integrity verified on download; signature verified if signing key available.
- **Env var injection.** Scripts access the binary via `{SLUG}_BIN` environment variable (e.g., `GWS_BIN=/path/to/gws`).
- **Coexists with embedded binaries.** Skills can still embed binaries in `.napp` archives via `RuntimeKind::Binary`. Plugins are for shared/heavy binaries.

**Why not just embed binaries?**
- 3 skills bundling `gws` = 3 copies of the same binary
- Platform-specific binaries require separate `.napp` archives per platform per skill
- Plugins solve both: download once, share everywhere, one `.napp` per skill regardless of platform

---

## 2. Architecture & Crate Placement

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

## 3. Plugin Types

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
}
```

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

## 4. PluginStore

**Source:** `crates/napp/src/plugin.rs`

Core struct managing the plugin lifecycle. Constructed once at startup, stored as `Arc<PluginStore>` on AppState.

```rust
pub struct PluginStore {
    plugins_dir: PathBuf,                                           // <data_dir>/nebo/plugins/
    signing_key: Option<Arc<SigningKeyProvider>>,                    // ED25519 verification
    manifests: Arc<tokio::sync::RwLock<HashMap<String, PluginManifest>>>,  // Cache keyed by "slug:version"
    downloading: Arc<tokio::sync::Mutex<HashSet<String>>>,          // Concurrent download dedup
}
```

### Methods

| Method | Async | Description |
|--------|-------|-------------|
| `new(plugins_dir, signing_key)` | No | Constructor |
| `plugins_dir()` | No | Returns root storage path |
| `resolve(slug, version_range)` | No | Local-only semver resolution → `Option<PathBuf>` |
| `ensure(slug, version_range, download_fn)` | Yes | Resolve locally or download, returns binary path |
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

### find_binary_in_version_dir()

Two-step binary discovery:

1. Read `plugin.json` → look up current platform → check if `binary_name` file exists
2. Fallback: scan directory for first executable file (Unix: mode & 0o111; Windows: .exe/.bat/.cmd)

---

## 5. SKILL.md Integration

Skills declare plugin dependencies in YAML frontmatter:

```yaml
---
name: google-workspace
description: Manage Google Workspace (Gmail, Calendar, Drive)
plugins:
  - name: gws
    version: ">=1.2.0"
  - name: ffmpeg
    version: ">=5.0.0"
    optional: true
---
```

- `name` matches the plugin's `slug` in NeboLoop
- `version` is a semver range string (default `"*"`)
- `optional: true` means the skill loads even if this plugin isn't installed

The `plugins` field is parsed into `Vec<PluginDependency>` on the `Skill` struct.

---

## 6. Skill Loader Integration

**Source:** `crates/tools/src/skills/loader.rs`

The `Loader` struct has an optional `plugin_store: Option<Arc<napp::plugin::PluginStore>>` field, set via the builder method `with_plugin_store()`.

During `load_all()`, after scanning all skill directories, the loader calls `verify_dependencies()` which now checks plugin dependencies:

```
For each loaded skill:
  For each plugin in skill.plugins:
    If plugin.optional → skip
    If plugin_store.resolve(plugin.name, plugin.version) → None:
      Drop skill with warning: "skill skipped: missing plugin"
```

Skills with missing **required** plugin dependencies are removed from the loaded set. Optional plugins are silently skipped.

The hot-reload watcher (`watch()`) clones the `plugin_store` Arc and passes it to the reload closure, ensuring plugin checks happen on every reload cycle.

---

## 7. ExecuteTool Integration

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

## 8. Code System (PLUG-XXXX-XXXX)

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
5. Call `plugin_store.ensure()` with a download callback that queries `api.get_plugin()` and `api.download_plugin_binary()`
6. On success: broadcast `plugin_installed` event, reload skill loader
7. On failure: broadcast `plugin_error` event

Both the WebSocket handler and the REST `POST /api/v1/codes/redeem` handler dispatch to `handle_plugin_code()`.

---

## 9. Dependency Cascade

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

## 10. NeboLoop API

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

## 11. AppState Wiring

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

## 12. Sandbox Policy

**Source:** `crates/tools/src/sandbox_policy.rs`

No sandbox policy changes were needed. The sandbox uses a **deny-list** model for filesystem reads (not an allow-list). Since `<data_dir>/nebo/plugins/` is not on any deny list, plugin binaries are readable by default.

Plugin binaries are executed as subprocesses by the skill's script (e.g., `$GWS_BIN --list-emails`), not by the sandbox directly. The script subprocess inherits the sandbox profile, which allows read access to the plugin binary path.

---

## 13. Storage Layout

```
<data_dir>/
  nebo/
    skills/           # Marketplace skills (existing)
    plugins/          # Plugin binaries (NEW)
      gws/
        1.2.0/
          plugin.json # Cached PluginManifest (JSON, camelCase)
          gws         # Native binary (chmod 755)
        1.3.0/
          plugin.json
          gws
      ffmpeg/
        5.0.0/
          plugin.json
          ffmpeg
```

- Directory per slug, subdirectory per version
- `plugin.json` = serialized `PluginManifest` with full platform map
- Binary = native executable, platform-specific
- `.quarantined` marker file = quarantined version (binary deleted, manifest preserved)
- Multiple versions can coexist (different skills may require different ranges)

---

## 14. Concurrency Model

| Concern | Solution |
|---------|----------|
| Manifests cache | `tokio::sync::RwLock<HashMap>` — async reads, `ensure()` holds write across `.await` |
| Concurrent downloads | `downloading: Arc<tokio::sync::Mutex<HashSet<String>>>` — check-then-insert dedup. Second caller polls `resolve()` every 1s for 30s |
| GC vs reload race | GC takes `HashSet<String>` snapshot (not `&[Skill]`), snapshot-then-release pattern |
| `resolve()` is sync | Local filesystem only — no async needed, safe to call from sync contexts |

---

## 15. Platform Detection

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

## 16. Precedence Rule

When a skill has BOTH an embedded binary (`RuntimeKind::Binary` from `.napp`) AND a `plugins:` dependency for the same tool:

- **Embedded binary wins.** The `execute_tool.rs` binary detection (lines ~416-443) runs BEFORE plugin env var injection.
- Plugin env vars are injected for scripts to reference, not for the execute tool's own binary detection.
- This is the expected behavior: embedded = bundled with skill, plugin = available for scripts.

---

## 17. WebSocket Events

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

## 18. Edge Cases

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

## 19. Key Files

| File | Lines | What |
|------|-------|------|
| `crates/napp/src/plugin.rs` | 777 | Core module: types, PluginStore, helpers, tests |
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

## Cross-References

- **Skills SME:** `docs/sme/SKILLS_SME.md` — SKILL.md format, loader, ExecuteTool, sandbox
- **Publisher's Guide:** `docs/publishers-guide/plugins.md` — how to create and publish plugins
- **Packaging:** `docs/publishers-guide/packaging.md` — .napp archives, qualified names, install codes
- **Security SME:** `docs/sme/SECURITY.md` — signing, verification, sandbox
