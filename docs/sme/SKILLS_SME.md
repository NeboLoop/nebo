# Skill System — Rust SME Reference

> Definitive reference for the Nebo Rust skill system. Covers the SKILL.md
> definition format, YAML frontmatter fields, validation rules, the Loader
> (filesystem scanning + hot-reload), the SkillTool (agent interface),
> script execution via ExecuteTool, sandbox policy, REST endpoints,
> marketplace installation, persistence, dependency cascade, and
> agent integration.

**Canonical spec:** [platform-taxonomy.md](../.archive/platform-taxonomy.md)

---

## Table of Contents

1. [System Overview](#1-system-overview)
2. [SKILL.md Definition Format](#2-skillmd-definition-format)
3. [Skill Struct & Validation](#3-skill-struct--validation)
4. [Frontmatter Parsing](#4-frontmatter-parsing)
5. [Skill Loader](#5-skill-loader)
6. [Hot-Reload Watcher](#6-hot-reload-watcher)
7. [SkillTool (Agent Interface)](#7-skilltool-agent-interface)
8. [ExecuteTool (Script Runtime)](#8-executetool-script-runtime)
9. [Sandbox Policy](#9-sandbox-policy)
10. [HTTP Endpoints](#10-http-endpoints)
11. [Marketplace Installation (SKIL-XXXX-XXXX)](#11-marketplace-installation-skil-xxxx-xxxx)
12. [Persistence (persist_skill_from_api)](#12-persistence-persist_skill_from_api)
13. [Dependency Cascade](#13-dependency-cascade)
14. [Agent Integration](#14-agent-integration)
15. [Filesystem & Package Storage](#15-filesystem--package-storage)
16. [Legacy YAML Format](#16-legacy-yaml-format)
17. [Resource Files](#17-resource-files)
18. [Integration Points](#18-integration-points)
19. [Constants & Defaults](#19-constants--defaults)
20. [Cross-Reference to Go Docs](#20-cross-reference-to-go-docs)

---

## 1. System Overview

A **Skill** is domain knowledge expressed as markdown. It sits at the base of the AGENT → WORK → SKILL hierarchy:

```
AGENT (schedule of intent)
  └─ WORKFLOW (procedure)
      └─ ACTIVITY (LLM-guided task)
          └─ SKILL (domain knowledge)
              └─ optional: scripts/, references/, assets/
```

> **Key principle:** Skills define what the agent needs to *know*. That knowledge may demand capabilities (runtimes, tools), but the skill itself is pure instruction.

**Key properties:**
- Skills are **filesystem-only** — no database tables (unlike workflows/roles)
- Three storage tiers: **bundled** (shipped with app) → **installed** (marketplace .napp) → **user** (loose files, highest priority)
- **Hot-reload** via filesystem watcher (debounced 1s)
- **Trigger matching** — case-insensitive substring match against user messages
- **Dependency verification** — skills with missing deps are dropped at load time
- **Platform filtering** — skills can declare target OS (empty = all platforms)
- 80%+ of marketplace skills are **pure markdown** — no runtime needed
- Skills with scripts execute via **ExecuteTool** using bundled `uv` (Python) or `bun` (JS/TS)
- **Agent Skills Standard** compliance (https://skill.md) plus Nebo extensions

---

## 2. SKILL.md Definition Format

**Source:** `crates/tools/src/skills/skill.rs`

A skill is a markdown file with YAML frontmatter:

```markdown
---
name: web-research
description: Deep web research and summarization
version: "1.0.0"
author: nebo
license: Apache-2.0
compatibility: Requires python3
allowed-tools: Web(search:*) Shell
tags: [research, web]
platform: [macos, linux]
capabilities: [python, storage]
triggers:
  - research
  - look up
  - find information
dependencies:
  - base-research-skill
priority: 10
max_turns: 5
metadata:
  nebo:
    emoji: "mag"
---
# Web Research Skill

You are a research assistant. When activated, focus on:
1. Using web(action: "search", ...) for authoritative sources
2. Synthesizing findings into clear summaries
```

### Frontmatter Fields

**Agent Skills Standard fields:**

| Field | Type | Default | Constraints | Purpose |
|-------|------|---------|-------------|---------|
| **name** | string (required) | — | lowercase + digits + hyphens only; no leading/trailing/consecutive hyphens; 1–64 chars | Unique skill identifier |
| **description** | string (required) | — | 1–1024 chars | What the skill does |
| **license** | string | `""` | — | License name or reference |
| **compatibility** | string | `""` | max 500 chars | System requirements (python3, poppler-utils, etc.) |
| **allowed-tools** | string | `""` | space-delimited | Pre-approved tools (experimental) |
| **metadata** | object | `{}` | — | Arbitrary key-value extensions |

**Nebo extension fields:**

| Field | Type | Default | Purpose |
|-------|------|---------|---------|
| **version** | string | `"1.0.0"` | Semantic versioning |
| **author** | string | `""` | Publisher/creator |
| **dependencies** | string[] | `[]` | Other skills this skill requires (by name) |
| **tags** | string[] | `[]` | Categorization tags |
| **platform** | string[] | `[]` | Target OS: `"macos"`, `"linux"`, `"windows"` (empty = all) |
| **capabilities** | string[] | `[]` | Runtime needs: `"python"`, `"typescript"`, `"storage"`, `"network"`, `"vision"` |
| **triggers** | string[] | `[]` | Keywords for automatic activation (case-insensitive substring match) |
| **priority** | i32 | `0` | Higher = matches first when multiple skills trigger |
| **max_turns** | i32 | `0` | Max conversation turns (0 = unlimited) |

### Markdown Body

Everything after the closing `---` is the **template** — the actual skill instructions. This is injected into the agent's system prompt when the skill is active, or into a workflow activity's prompt when referenced.

---

## 3. Skill Struct & Validation

**Source:** `crates/tools/src/skills/skill.rs`

```rust
pub struct Skill {
    // Agent Skills Standard fields
    pub name: String,
    pub description: String,
    pub license: String,
    pub compatibility: String,
    pub allowed_tools: String,              // alias: "allowed-tools"
    pub metadata: HashMap<String, serde_json::Value>,

    // Nebo extension fields
    pub version: String,                     // default: "1.0.0"
    pub author: String,
    pub dependencies: Vec<String>,
    pub tags: Vec<String>,
    pub platform: Vec<String>,
    pub triggers: Vec<String>,
    pub capabilities: Vec<String>,
    pub priority: i32,
    pub max_turns: i32,

    // Runtime fields (not serialized from YAML)
    pub template: String,                    // markdown body (#[serde(skip)])
    pub enabled: bool,                       // #[serde(skip)]
    pub source_path: Option<PathBuf>,        // #[serde(skip)]
    pub source: SkillSource,                 // Installed | User
    pub base_dir: Option<PathBuf>,           // #[serde(skip)] — root directory
}

pub enum SkillSource {
    Installed,    // Bundled skills + NeboLoop marketplace (sealed .napp)
    User,         // User-created (loose files)
}
```

### Validation Rules

```rust
pub fn validate(&self) -> Result<(), String>
```

| Rule | Constraint |
|------|-----------|
| name required | Non-empty |
| name max length | 64 characters |
| name characters | Lowercase ASCII, digits, hyphens only |
| name no leading/trailing hyphen | Must not start or end with `-` |
| name no consecutive hyphens | Must not contain `--` |
| description required | Non-empty |
| description max length | 1024 characters |
| compatibility max length | 500 characters |

### Helper Methods

```rust
pub fn matches_platform(&self) -> bool       // empty platform[] = matches all
pub fn needs_sandbox(&self) -> bool          // true if capabilities contain "python" or "typescript"
pub fn matches_trigger(&self, msg: &str) -> bool  // case-insensitive substring match
pub fn list_resources(&self) -> Result<Vec<String>, String>  // walk base_dir, skip SKILL.md/manifest.json/hidden
pub fn read_resource(&self, rel_path: &str) -> Result<Vec<u8>, String>  // path traversal protected
```

---

## 4. Frontmatter Parsing

**Source:** `crates/tools/src/skills/skill.rs`

### split_frontmatter()

```rust
pub fn split_frontmatter(data: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String>
```

1. Trim leading whitespace
2. Verify starts with `---`
3. Skip past first newline after opening `---`
4. Find closing `\n---` on its own line
5. Return (frontmatter_bytes, body_bytes)

Errors if: no `---` at start, or no closing `---` found.

### parse_skill_md()

```rust
pub fn parse_skill_md(data: &[u8]) -> Result<Skill, String>
```

1. Split frontmatter from body
2. Deserialize YAML frontmatter into `Skill` struct via `serde_yaml`
3. Set `skill.template` to the body content
4. Call `skill.validate()`
5. Return validated skill

---

## 5. Skill Loader

**Source:** `crates/tools/src/skills/loader.rs`

### Loader Struct

```rust
pub struct Loader {
    bundled_dir: PathBuf,       // e.g. <data_dir>/bundled/skills/
    installed_dir: PathBuf,     // e.g. <data_dir>/nebo/skills/
    user_dir: PathBuf,          // e.g. <data_dir>/user/skills/
    skills: Arc<RwLock<HashMap<String, Skill>>>,
}
```

### load_all()

```rust
pub async fn load_all(&self) -> usize
```

Loading order (later tiers override earlier by name):

1. **Bundled** (`bundled_dir`) — `load_skills_from_dir()` — force `enabled = true`
2. **Installed** (`installed_dir`) — `load_skills_from_nested_dir()` — force `enabled = true`, uses `napp::reader::walk_for_marker()` for recursive SKILL.md discovery
3. **User** (`user_dir`) — `load_skills_from_dir()` — respects enabled/disabled state
4. **Legacy YAML** (`user_dir`) — `load_yaml_skills()` — flat `.yaml` / `.yaml.disabled` files, only if name not already loaded

After loading all tiers:
- `verify_dependencies()` — drops skills whose `dependencies[]` reference names not in the loaded set
- Store in `Arc<RwLock<HashMap<String, Skill>>>`
- Return total count

### Key API Methods

```rust
pub async fn get(&self, name: &str) -> Option<Skill>
pub async fn list(&self) -> Vec<Skill>                    // sorted by priority desc, then name asc
pub async fn match_triggers(&self, message: &str, max: usize) -> Vec<Skill>  // enabled only, sorted by priority
pub fn watch(&self) -> tokio::task::JoinHandle<()>        // hot-reload watcher
pub fn write_skill(&self, name: &str, content: &str) -> Result<PathBuf, String>
pub fn resolve_user_skill_path(&self, name: &str) -> Option<PathBuf>
pub fn bundled_dir(&self) -> &Path
pub fn user_dir(&self) -> &Path
pub fn installed_dir(&self) -> &Path
```

### Loading Internals

**load_skills_from_dir()** — Reads immediate subdirectories, looks for SKILL.md (case-insensitive) or SKILL.md.disabled. Skips platform mismatches.

**load_skills_from_nested_dir()** — Uses `napp::reader::walk_for_marker(dir, "SKILL.md")` for recursive discovery. Handles extracted .napp directory trees like `@org/skills/name/1.0.0/SKILL.md`.

**load_yaml_skills()** — Backward compatibility. Reads flat `.yaml` and `.yaml.disabled` files. Creates `Skill` with `description: "YAML skill (legacy format)"` and file content as `template`.

### Dependency Verification

```rust
fn verify_dependencies(loaded: &mut HashMap<String, Skill>)
```

Builds a set of all loaded skill names. Retains only skills whose every dependency exists in that set. Logs `skill skipped: missing dependency` for dropped skills.

---

## 6. Hot-Reload Watcher

**Source:** `crates/tools/src/skills/loader.rs` — `Loader::watch()`

### Setup

- Uses `notify::RecommendedWatcher` with 2s poll interval
- Watches `user_dir` and `installed_dir` recursively (not bundled — those don't change)

### Trigger Conditions

Only reloads on Create/Modify/Remove events where the path matches:
- `SKILL.md` (case-insensitive)
- `*.yaml` or `*.yaml.disabled`
- `*.napp`
- Files in ancestor directories named: `scripts`, `references`, `assets`, `examples`, `agents`, `core`

### Debounce

1-second debounce — ignores events within 1s of the last reload.

### Reload Algorithm

Full re-scan: same algorithm as `load_all()` (bundled → installed → user → yaml → verify deps).

---

## 7. SkillTool (Agent Interface)

**Source:** `crates/tools/src/skill_tool.rs`

The `SkillTool` is the agent-facing tool for skill management. **Always registered** (core tool). Does **not** require approval.

```rust
pub struct SkillTool {
    loader: Arc<Loader>,
    store: Option<Arc<db::Store>>,     // required for "install" action
}
```

Tool name: `"skill"`

### Actions

| Action | Required Params | Purpose |
|--------|----------------|---------|
| `catalog` / `list` | — | List all skills with status, source, triggers, capabilities, resource count |
| `help` | `name` | Show full SKILL.md content + metadata + resource listing |
| `browse` | `name`, `path?` | List resource files with sizes, optionally filtered by directory prefix |
| `read_resource` | `name`, `path` | Read a specific resource file (path-traversal protected) |
| `load` | `name` | Enable skill: rename `.yaml.disabled` → `.yaml` or `SKILL.md.disabled` → `SKILL.md` |
| `unload` | `name` | Disable skill: rename `.yaml` → `.yaml.disabled` or `SKILL.md` → `SKILL.md.disabled` |
| `create` | `name`, `content` | Create new skill. If content starts with `---`: writes `{name}/SKILL.md`. Otherwise: `{name}.yaml` |
| `update` | `name`, `content` | Update existing skill. Rejects installed (marketplace) skills as read-only |
| `delete` | `name` | Delete user skill (directory + yaml files). Rejects installed skills |
| `install` | `code` | Install from marketplace (must start with `SKIL-`). Calls NeboLoop API, persists, reloads |
| `configure` | `name`, `key`, `value` | Set a secret/API key for a skill (encrypted at rest) |
| `secrets` | `name` | List declared secrets for a skill and their configuration status |
| `featured` | — | List enabled skills with non-empty capabilities (top 10) |
| `popular` | — | List enabled skills sorted by capability count (top 10) |
| `reviews` | `name` | Placeholder — returns "no reviews available" (marketplace API stub) |

### Schema

```json
{
  "type": "object",
  "properties": {
    "action": { "type": "string", "enum": ["catalog", "help", "browse", "read_resource", "load", "unload", "create", "update", "delete", "install", "configure", "secrets", "featured", "popular", "reviews"] },
    "name": { "type": "string", "description": "Skill name (slug)" },
    "content": { "type": "string", "description": "Skill YAML content (for create/update)" },
    "path": { "type": "string", "description": "Relative path for browse filter or resource read" },
    "code": { "type": "string", "description": "Marketplace code for install (e.g. SKIL-XXXX-XXXX)" },
    "key": { "type": "string", "description": "Secret/API key name for configure action (e.g. BRAVE_API_KEY)" },
    "value": { "type": "string", "description": "Secret value for configure action" }
  },
  "required": ["action"]
}
```

### Catalog Output Format

```
{count} skills:
- {name} [{enabled|disabled}|{nebo|user}] — {description} [caps: python, storage] (3 resource files) (triggers: research, look up)
```

### Protection Rules

- **Installed (marketplace) skills** are read-only: `update` and `delete` are rejected with a clear error message
- **Content newline normalization**: `create` replaces literal `\\n` with real newlines (LLMs often escape them)
- **help fallback**: If skill not in loader, falls back to reading raw file from `user/skills/` directory

---

## 8. ExecuteTool (Script Runtime)

**Source:** `crates/tools/src/execute_tool.rs`

Runs Python/TypeScript/JavaScript scripts bundled with skills. **Requires approval.**

```rust
pub struct ExecuteTool {
    loader: Arc<Loader>,
    plan_tier: Arc<RwLock<String>>,
    sandbox: Option<Arc<SandboxManager>>,
}
```

Tool name: `"execute"`

Only registered when both `skill_loader` and `plan_tier` are available.

### Schema

```json
{
  "type": "object",
  "properties": {
    "skill": { "type": "string", "description": "Name of the skill containing the script" },
    "script": { "type": "string", "description": "Relative path (e.g. 'scripts/recalc.py')" },
    "args": { "type": "object", "description": "Arguments passed as SKILL_ARGS env var" },
    "timeout": { "type": "integer", "description": "Timeout in seconds", "default": 30 }
  },
  "required": ["skill", "script"]
}
```

### Execution Pipeline

```
1. Look up skill by name from Loader
2. Detect language from file extension:
   .py  → "python"
   .ts  → "typescript"
   .js  → "javascript"
3. Find runtime (see below)
4. If runtime found → execute_local()
5. If no runtime + paid tier (pro/team/enterprise) → cloud sandbox (stubbed)
6. If neither → structured error with install/upgrade options
```

### Runtime Resolution

```rust
enum RuntimeKind {
    Uv(PathBuf),       // Bundled uv in /tmp/nebo-runtimes/
    Bun(PathBuf),      // Bundled bun in /tmp/nebo-runtimes/
    System(PathBuf),   // System PATH
}
```

Resolution order:

| Language | Step 1 (bundled) | Step 2 (system) |
|----------|-----------------|-----------------|
| python | `/tmp/nebo-runtimes/uv` → `uv run script.py` | `python3` or `python` via `which` |
| typescript/javascript | `/tmp/nebo-runtimes/bun` → `bun run script.ts` | `node` via `which` (+ `tsx` for TS) |

### Local Execution (execute_local)

1. Create temp directory
2. Extract ALL skill resources into it (preserves directory structure for multi-file imports)
3. Build base command string: `{runtime} run {script_path}`
4. Wrap with OS sandbox if `SandboxManager` available (fallback to bare execution on sandbox error)
5. Execute via `sh -c` with:
   - `SKILL_ARGS` env var (JSON-encoded arguments)
   - `current_dir` set to temp directory
   - piped stdout/stderr
6. Apply timeout (default 30s, configurable)
7. Post-execution: sandbox cleanup, annotate sandbox violations in stderr
8. Return stdout as success, or exit code + stdout/stderr as error

### Cloud Sandbox (Stubbed)

For paid tiers (pro/team/enterprise): `POST {janus_url}/v1/execute` — not yet implemented. Returns helpful error directing users to install runtime locally.

---

## 8b. Binary/Executable Skills

**Source:** `crates/tools/src/execute_tool.rs`, `crates/napp/src/napp.rs`, `crates/napp/src/reader.rs`

Nebo supports skills that bundle **pre-compiled native executables**. The agent calls these binaries according to the skill's SKILL.md instructions, passing arguments via environment variables and capturing stdout/stderr.

### Concept

A binary skill is a standard skill (SKILL.md + resources) that additionally includes a native executable. The SKILL.md markdown body instructs the agent *when* and *how* to call the binary. The execute tool handles the actual invocation.

```
SKILL.md              → Agent instructions (when to call, what args to pass)
binary (or bin/*)     → Pre-compiled native executable
scripts/              → Optional helper scripts
references/           → Optional reference data
```

> **Design principle:** The SKILL.md is the *brain* — it teaches the agent the domain knowledge. The binary is the *muscle* — it performs computation the LLM cannot do (PDF generation, image processing, data crunching). The agent reads the SKILL.md instructions, decides when to invoke the binary, constructs the args, and interprets the output.

### .napp Binary Packaging

Binary skills are distributed as sealed `.napp` archives (tar.gz). The archive enforces strict content rules:

**Allowed files in .napp** (`crates/napp/src/napp.rs:16-31`):

```
manifest.json         — Required. Package identity + metadata
binary                — Native executable (root-level, legacy convention)
app                   — Alternative binary name (root-level)
bin/*                 — Binary directory (new convention, multiple executables)
scripts/*             — Executable scripts (.py, .ts, .js)
signatures.json       — ED25519 signatures for verification
SKILL.md              — Skill definition
ui/*                  — UI assets (5MB max per file)
```

**Size limits:**

| Entry | Max Size |
|-------|----------|
| `binary` / `app` | 500 MB |
| `ui/*` files | 5 MB each |
| All other metadata | 1 MB |

**Security enforcement (`napp.rs:54-80`):**
- Path traversal rejected (`..` or leading `/`)
- Symlinks and hardlinks rejected (tar entry type check)
- Only ALLOWED_FILES entries permitted (unexpected files → extraction error)
- Canonical path verification (escape detection)

### Binary Format Validation

Native executables are validated at extraction time via magic byte checks (`napp.rs:161-188`):

| Format | Magic Bytes | Platform |
|--------|------------|----------|
| ELF | `7F 45 4C 46` | Linux |
| Mach-O 32-bit | `FE ED FA CE` | macOS |
| Mach-O 64-bit | `FE ED FA CF` | macOS |
| Mach-O 32 (swapped) | `CE FA ED FE` | macOS |
| Mach-O 64 (swapped) | `CF FA ED FE` | macOS |
| Universal (FAT) | `CA FE BA BE` | macOS (multi-arch) |
| PE/COFF | `4D 5A` | Windows |

**Rejected:**
- Shebang scripts (`#!`) → error: "shebang scripts not allowed — compiled binaries only"
- Unknown formats → error: "unrecognized binary format — only native executables allowed"
- Files < 4 bytes → error: "binary too small"

### Binary Extraction & Permissions

When a `.napp` is extracted (`reader.rs:85-110`, `reader.rs:157-217`):

1. Binary content is written to disk
2. On Unix: permissions set to `0o755` (owner rwx, group rx, others rx)
3. Applies to entries named: `binary`, `app`, `bin/*`, `scripts/*`
4. Extraction is idempotent via `extract_napp_alongside()` — skips if directory already exists

### Binary Locations in Skill Directory

After .napp extraction, binaries reside in the skill's `base_dir`:

```
# New convention (preferred): bin/ directory
nebo/skills/@acme/skills/pdf-tool/1.0.0/
├── SKILL.md
├── manifest.json
├── bin/
│   └── nebo-pdf          ← binary at bin/nebo-pdf
└── references/
    └── templates/

# Legacy convention: root-level "binary"
nebo/skills/@acme/skills/xlsx-proc/1.0.0/
├── SKILL.md
├── manifest.json
└── binary                ← binary at root
```

### Platform Filtering

Skills declare target platforms in SKILL.md frontmatter:

```yaml
platform: [macos, linux]    # Only load on macOS and Linux
platform: []                # Empty = all platforms (default)
```

The loader checks `skill.matches_platform()` at load time. Skills for the wrong platform are silently skipped — they never appear in the catalog.

For cross-platform binary skills, publishers create separate `.napp` archives per platform (each containing the correct native binary) and upload them to the marketplace tagged by platform.

### Binary Execution Pipeline

**Entry point:** `ExecuteTool.execute_dyn()` (`execute_tool.rs:392-493`)

```
Agent calls: execute(skill: "pdf-tool", script: "bin/nebo-pdf", args: {...})
  │
  ├─[1] Lookup skill by name from Loader
  │
  ├─[2] Binary detection (lines 416-417):
  │     script_path == "binary" OR script_path.starts_with("bin/")
  │
  ├─[3] Binary lookup (lines 419-442):
  │     ├─ Try: skill.base_dir / script_path   (e.g., base_dir/bin/nebo-pdf)
  │     ├─ Fallback: skill.base_dir / "binary" (only if script_path == "binary")
  │     └─ If neither exists → error: "no binary at '...' for this platform"
  │
  └─[4] execute_local(RuntimeKind::Binary, ...) (lines 219-345):
        ├─ Create temp directory
        ├─ Extract ALL skill resources into temp dir
        │   (binary may need reference files, templates, data)
        ├─ Verify script exists in temp dir
        ├─ Build command: raw binary path (no runtime launcher)
        │   RuntimeKind::Binary → script_path.to_string() (line 213)
        ├─ Sandbox wrapping (if SandboxManager available):
        │   ├─ build_sandbox_config(skill, tmp_dir)
        │   ├─ wrap_with_sandbox_opts(cmd)
        │   └─ Fallback to bare execution on sandbox error
        ├─ Inject SKILL_ARGS env var (JSON-encoded arguments)
        ├─ Inject declared secrets as env vars
        ├─ Execute via sh -c with:
        │   ├─ cwd = temp directory
        │   ├─ stdout piped
        │   ├─ stderr piped
        │   └─ timeout (default 30s, configurable)
        ├─ Post-execution sandbox cleanup
        └─ Return: stdout as success, or exit code + stderr as error
```

### Binary Communication Protocol

Binaries communicate with the agent through a simple stdio protocol:

| Channel | Direction | Purpose |
|---------|-----------|---------|
| **SKILL_ARGS** env var | Agent → Binary | JSON-encoded arguments object |
| **Secret env vars** | Agent → Binary | Decrypted API keys (e.g., `BRAVE_API_KEY`) |
| **stdout** | Binary → Agent | Result content (text, JSON, file paths) |
| **stderr** | Binary → Agent | Diagnostics, warnings, progress (appended to result) |
| **exit code** | Binary → Agent | 0 = success, non-zero = error |

The agent passes arguments via the `SKILL_ARGS` environment variable as a JSON string. The binary reads this, performs its work, and writes results to stdout. The agent receives the stdout content and interprets it according to the SKILL.md instructions.

### Example: Binary Skill SKILL.md

```markdown
---
name: pdf-generator
description: Generate and manipulate PDF documents
version: "1.0.0"
platform: [macos, linux]
capabilities: [storage]
triggers:
  - create pdf
  - generate pdf
  - pdf document
metadata:
  secrets:
    - key: LICENCE_KEY
      label: "PDF Engine License"
      required: false
---
# PDF Generator

You have access to a PDF generation binary. Use it to create professional
PDF documents from structured specifications.

## How to Call

Use the execute tool with this skill's binary:

```
execute(skill: "pdf-generator", script: "bin/nebo-pdf", args: {
  "command": "create",
  "title": "Document Title",
  "content": [
    {"type": "heading", "text": "Section 1"},
    {"type": "paragraph", "text": "Body text here..."},
    {"type": "table", "headers": ["Col A", "Col B"], "rows": [["1", "2"]]}
  ],
  "output": "report.pdf"
})
```

## Commands

| Command | Args | Description |
|---------|------|-------------|
| `create` | title, content[], output | Create new PDF |
| `merge` | files[], output | Merge multiple PDFs |
| `extract` | file, pages | Extract page range |

## Output

The binary writes the output file to the working directory and returns
the filename on stdout. Use this path to reference the generated file.
```

### Sandbox Behavior for Binaries

Binary execution is subject to the same sandbox policy as script execution:

- **Filesystem:** Always denied: `~/.ssh`, `~/.gnupg`, `~/.aws/credentials`, `~/.config/gcloud`. Always allowed: temp work dir, `/dev/stdout`, `/dev/stderr`, `/tmp/nebo`. If capability `storage`: also writable data dir.
- **Network:** Blocked unless capability `network` is declared. If declared: package registries + `metadata.allowed_domains`.
- **Fallback:** If sandbox wrapping fails, binary runs unsandboxed with a warning log.

### Key Differences: Binary vs Script Skills

| Aspect | Script Skill | Binary Skill |
|--------|-------------|-------------|
| **Runtime** | Requires Python/Node.js/Bun | Self-contained native executable |
| **Detection** | `.py`, `.ts`, `.js` extension | `"binary"` or `"bin/*"` path |
| **Launcher** | `uv run`, `bun run`, `python` | Direct execution (no launcher) |
| **Portability** | Cross-platform (interpreted) | Platform-specific (compile per OS) |
| **Size** | Typically KB | Up to 500 MB |
| **Validation** | Extension-based language detection | Magic byte format verification |
| **Dependencies** | May need package install (pip/npm) | Statically linked, zero deps |
| **Performance** | Interpreter overhead | Native speed |

### Current Limitations

1. **No `bin/` in .napp ALLOWED_FILES:** The secure extractor (`napp.rs:16-31`) only allows `binary`, `app`, and `ui/*`. The `bin/` directory pattern is supported by `reader.rs` (extract_all sets +x on `bin/*`) and `execute_tool.rs` (detects `bin/` prefix), but the strict `extract_napp()` allowlist does NOT include `bin/*`. This means:
   - `.napp` archives using the secure extractor cannot contain `bin/` directories
   - The `bin/` convention works with `extract_all()` / `extract_napp_alongside()` (used by skill install path)
   - **TODO:** Add `bin/*` to ALLOWED_FILES in `napp.rs` for full consistency

2. **Single binary per .napp (secure path):** The secure extractor allows exactly one binary (`binary` or `app`). Multiple executables require the `bin/` directory convention (via extract_all path only).

3. **No platform-specific binary naming:** The archive contains a single `binary` — there's no `binary.darwin-arm64` convention. Platform targeting is handled by publishing separate `.napp` archives per platform.

4. **No stdin pipe:** Binaries receive input only via env vars. No streaming stdin support.

5. **Cloud sandbox not wired:** `POST {janus_url}/v1/execute` is stubbed (execute_tool.rs:467). Binary skills currently require local execution only.

---

## 9. Sandbox Policy

**Source:** `crates/tools/src/sandbox_policy.rs`

Translates skill capabilities into per-execution sandbox configuration.

```rust
pub fn build_sandbox_config(skill: &Skill, work_dir: &Path) -> SandboxRuntimeConfig
```

### Always Denied (Read)

```
~/.ssh/
~/.gnupg/
~/.aws/credentials
~/.config/gcloud
```

### Always Allowed (Write)

```
{work_dir}          — Isolated temp directory
/dev/stdout
/dev/stderr
/tmp/nebo
```

### Capability Mapping

| Capability | Filesystem | Network |
|-----------|-----------|---------|
| *(base)* | write: work_dir, /dev/std*, /tmp/nebo | all blocked |
| `storage` | + write: nebo data dir | — |
| `network` | — | + pypi.org, files.pythonhosted.org, registry.npmjs.org, npm.pkg.github.com |

### Network: Extra Domains

If a skill has `network` capability and declares `metadata.allowed_domains` as a JSON array, those domains are added to the allowed list:

```yaml
capabilities: [network]
metadata:
  allowed_domains: ["api.example.com", "cdn.example.com"]
```

---

## 9b. Skill Secrets

**Source:** `crates/tools/src/skills/skill.rs`, `crates/tools/src/execute_tool.rs`, `crates/db/src/queries/settings.rs`

Skills can declare required secrets (API keys, tokens) in their SKILL.md frontmatter:

```yaml
metadata:
  secrets:
    - key: BRAVE_API_KEY
      label: "Brave Search API Key"
      hint: "https://brave.com/search/api/"
      required: true
    - key: BRAVE_REGION
      label: "Default region"
      required: false
```

### SecretDeclaration Struct

```rust
pub struct SecretDeclaration {
    pub key: String,       // Environment variable name
    pub label: String,     // Human-readable label for UI
    pub hint: String,      // Help text (e.g., URL to get the key)
    pub required: bool,    // Whether the skill needs this to function
}
```

Parsed via `Skill::secrets()` which reads `metadata.secrets` as a JSON array.

### Secret Storage

Secrets are encrypted with AES-256-GCM via `auth::credential::encrypt()` and stored in the `plugin_settings` table with `is_secret = 1`:

```rust
// Store
store.set_skill_secret(skill_name, key, encrypted_value)
// Retrieve
store.get_skill_secret(skill_name, key) → Option<encrypted_value>
// List
store.list_skill_secrets(skill_name) → Vec<(key, encrypted_value)>
// Delete
store.delete_skill_secret(skill_name, key)
```

The `plugin_settings` table uses `plugin_id = "skill-{name}"`, created on-demand via `ensure_skill_plugin()`.

### Secret Injection (ExecuteTool)

When `ExecuteTool` runs a skill's script:

1. `resolve_secrets(store, skill)` reads all declared secrets
2. Decrypts each via `auth::credential::decrypt()`
3. Injects as **environment variables** on the child process (`cmd.env(key, value)`)
4. If any **required** secret is missing → execution is **blocked** with a structured error message listing what's missing and how to configure

### Configuration Methods

**Agent tool:**
```
skill(action: "configure", name: "brave-search", key: "BRAVE_API_KEY", value: "BSA...")
skill(action: "secrets", name: "brave-search")
```

**REST API:**
```
GET    /api/v1/skills/:name/secrets         → list declarations + configured status
PUT    /api/v1/skills/:name/secrets         → set a secret (body: {key, value})
DELETE /api/v1/skills/:name/secrets/:key    → remove a secret
```

**list_extensions enrichment:**
The `GET /api/v1/extensions` endpoint now includes `secrets` array and `needsConfiguration` boolean for each skill that declares secrets.

---

## 10. HTTP Endpoints

**Source:** `crates/server/src/handlers/skills.rs`

All endpoints operate on `user/skills/` directory only (no installed/marketplace skills via REST).

| Method | Path | Handler | Purpose |
|--------|------|---------|---------|
| GET | `/api/v1/extensions` | `list_extensions` | List skill files with metadata (backward compat) |
| POST | `/api/v1/skills` | `create_skill` | Create new skill (name + content) |
| GET | `/api/v1/skills/:name` | `get_skill` | Read skill content by name |
| GET | `/api/v1/skills/:name/content` | `get_skill_content` | Alias for get_skill |
| PUT | `/api/v1/skills/:name` | `update_skill` | Update skill content |
| DELETE | `/api/v1/skills/:name` | `delete_skill` | Delete skill (directory + yaml files) |
| POST | `/api/v1/skills/:name/toggle` | `toggle_skill` | Enable/disable (rename dir or yaml file) |
| POST | `/api/v1/codes` | `submit_code` | Redeem marketplace code (multi-type dispatch) |
| GET | `/api/v1/skills/:name/secrets` | `list_skill_secrets` | List declared secrets + configured status |
| PUT | `/api/v1/skills/:name/secrets` | `set_skill_secret` | Store encrypted secret (body: `{key, value}`) |
| DELETE | `/api/v1/skills/:name/secrets/:key` | `delete_skill_secret` | Remove a configured secret |

### POST /api/v1/skills (Create)

```json
// Request
{ "name": "my-skill", "content": "---\nname: my-skill\n..." }

// Response
{ "name": "my-skill", "path": "/path/to/user/skills/my-skill/SKILL.md" }
```

Delegates to `tools::skills::write_skill()` — auto-detects SKILL.md vs .yaml from content.

### GET /api/v1/extensions (List)

Returns metadata including source, description, version, triggers where parseable:

```json
{
  "extensions": [
    {
      "name": "research",
      "enabled": true,
      "path": "/path/to/SKILL.md",
      "source": "user",
      "description": "Deep research",
      "version": "1.0.0",
      "triggers": ["research", "look up"]
    }
  ]
}
```

### POST /api/v1/skills/:name/toggle

Two toggle mechanisms:
1. **Directory toggle**: `my-skill/` ↔ `my-skill.disabled/`
2. **YAML toggle**: `my-skill.yaml` ↔ `my-skill.yaml.disabled`

---

## 11. Marketplace Installation (SKIL-XXXX-XXXX)

**Source:** `crates/server/src/codes.rs`

### Code Detection

```rust
pub fn detect_code(prompt: &str) -> Option<(CodeType, &str)>
```

Code format: `SKIL-XXXX-XXXX` where XXXX = 4 Crockford Base32 characters (charset: `0123456789ABCDEFGHJKMNPQRSTVWXYZ`, no I/L/O/U).

Case-insensitive detection, whitespace-trimmed.

### Installation Flow (handle_skill_code)

```
User enters: "SKIL-RFBM-XCYT"
  ↓
1. detect_code() → (CodeType::Skill, "SKIL-RFBM-XCYT")
2. Broadcast "code_processing" event
3. build_api_client() → NeboLoopApi (requires NeboLoop connection)
4. api.install_skill(code) → CodeRedeemResponse
   ├─ status: "payment_required" → return checkout_url
   └─ status: "installed" → continue
5. persist_skill_from_api(api, artifact_id, name, code) → skill_dir
6. skill_loader.load_all() → hot-reload, skill appears in catalog
7. Spawn background task:
   ├─ Read SKILL.md from skill_dir
   ├─ parse_skill_md() → extract dependencies
   └─ resolve_cascade(deps) → auto-install child deps
8. Broadcast "code_result" (success/failure)
9. Broadcast "chat_complete" (reset frontend loading state)
```

### Two Entry Points

Skills can be installed via:
1. **Chat interception** — code detected in user message before it reaches the agent runner (`handle_code()` in `codes.rs`)
2. **SkillTool** — agent calls `skill(action: "install", code: "SKIL-XXXX-XXXX")` (`skill_tool.rs`)
3. **REST API** — `POST /api/v1/codes` with `{ "code": "SKIL-XXXX-XXXX" }` (`submit_code()` in `codes.rs`)

All three converge on the same NeboLoop API call and persistence logic.

---

## 12. Persistence (persist_skill_from_api)

**Source:** `crates/tools/src/lib.rs`

```rust
pub async fn persist_skill_from_api(
    api: &NeboLoopApi,
    artifact_id: &str,
    name: &str,
    code: &str,
) -> Result<PathBuf, String>
```

### Algorithm

```
1. api.get_skill(artifact_id) → SkillDetail
2. Determine dir_name (slug or name) and version
3. If download_url exists:
   a. Create nebo/skills/{dir_name}/
   b. Download .napp to nebo/skills/{dir_name}/{version}.napp
   c. extract_napp_alongside() → nebo/skills/{dir_name}/{version}/
   d. Return extracted directory
   e. On failure: fall through to loose files
4. Fallback (no download_url or download failed):
   a. Extract manifest text from API (or generate minimal SKILL.md)
   b. Write nebo/skills/{dir_name}/SKILL.md
   c. Write nebo/skills/{dir_name}/manifest.json
   d. Return skill directory
```

### Minimal SKILL.md Generation

When the API returns no manifest content:

```rust
fn generate_minimal_skill_md(name: &str, description: &str) -> String {
    format!("---\nname: {}\ndescription: {}\n---\n{}\n", name, description, description)
}
```

---

## 13. Dependency Cascade

**Source:** `crates/server/src/deps.rs`

### Extraction

```rust
pub fn extract_skill_deps(skill: &tools::skills::Skill) -> Vec<DepRef>
```

Maps each entry in `skill.dependencies[]` to a `DepRef { dep_type: Skill, reference }`.

### Resolution

```
resolve_cascade(state, deps, visited):
  FOR EACH dep:
    1. Cycle check (skip if visited)
    2. is_installed()?
       ├─ Skill: check user/skills/{name}.yaml, user/skills/{name}/SKILL.md,
       │         nebo/skills/ (qualified path or walk_for_marker)
       └─ Yes → AlreadyInstalled, skip
    3. is_marketplace_ref()?
       ├─ Starts with @ or SKIL-/WORK-/AGNT- → yes
       └─ Simple name (no prefix) → Unresolvable (built-in)
    4. If autonomous mode:
       ├─ install_dep() → call NeboLoop API
       ├─ Broadcast "dep_installed"
       └─ Recurse into child deps
    5. If non-autonomous:
       ├─ Broadcast "dep_pending"
       └─ Mark PendingApproval (user must approve)
```

### Autonomy Modes

- **Autonomous** (`settings.autonomous_mode = 1`): Auto-install missing deps
- **Non-autonomous**: Mark deps as `PendingApproval`, broadcast event, wait for user approval
- **Force** (`POST /deps/approve`): Install regardless of autonomy setting

### Reference Types

| Format | Example | Handled |
|--------|---------|---------|
| Install code | `SKIL-RFBM-XCYT` | API install |
| Qualified name | `@nebo/skills/research@^1.0.0` | API install |
| Simple name | `web`, `research` | Marked Unresolvable (built-in) |

---

## 14. Agent Integration

### Skill Loading at Startup

```rust
// In AppState initialization
let skill_loader = Arc::new(Loader::new(bundled_dir, installed_dir, user_dir));
skill_loader.load_all().await;

// Register SkillTool and ExecuteTool in registry
registry.register(Box::new(SkillTool::new(skill_loader.clone())));
registry.register(Box::new(ExecuteTool::new(skill_loader.clone(), plan_tier, sandbox)));

// Start hot-reload watcher
tokio::spawn(skill_loader.watch());
```

### Trigger Matching

When an agent message arrives, the system can optionally match skill triggers:

```rust
let matches = skill_loader.match_triggers(message, 3).await;
// Returns up to 3 skills sorted by priority (highest first)
// Only considers enabled skills
// Case-insensitive substring matching against each skill's triggers array
```

### Workflow Activity Integration

Activities reference skills by name in `activity.skills[]`. The workflow engine:
1. Loads SKILL.md content via the `Loader`
2. Injects skill templates into the activity system prompt as a `## Skills` section
3. Each activity gets only the skills it declares — no global skill context bleed

### Tool Filtering

Skills tool (`"skill"`) is always in the core tool set:

```rust
const CORE_TOOLS: &[&str] = &["os", "web", "agent", "event", "message", "skill", "persona"];
```

---

## 15. Filesystem & Package Storage

### Directory Structure

```
{data_dir}/
├── bundled/skills/               # Shipped with app (lowest priority)
│   └── skill-name/
│       └── SKILL.md
│
├── nebo/skills/                  # Marketplace (sealed .napp archives)
│   └── @org/skills/name/
│       ├── 1.0.0.napp           # Sealed tar.gz archive
│       └── 1.0.0/               # Extracted directory
│           ├── SKILL.md
│           ├── manifest.json
│           ├── signatures.json   # ED25519 signatures (optional)
│           └── scripts/
│               └── run.py
│
└── user/skills/                  # User-created (highest priority)
    ├── my-skill/
    │   ├── SKILL.md
    │   ├── scripts/
    │   │   └── process.py
    │   └── references/
    │       └── guide.md
    ├── legacy.yaml               # Backward-compatible flat files
    └── disabled.yaml.disabled    # Disabled skills
```

### Priority Override

If a user skill has the same `name` as an installed skill, the user version wins:
```
bundled "research" → overridden by installed "research" → overridden by user "research"
```

### .napp Archive Contents

```
SKILL-XXXX-XXXX.napp (tar.gz)
├── manifest.json         # Package identity + metadata
├── SKILL.md              # Skill definition
├── signatures.json       # Code signing (optional)
├── scripts/              # Executable scripts
├── references/           # Reference documents
└── assets/               # Static assets
```

### Integrity Model

| Source | Read Method | Integrity |
|--------|------------|-----------|
| Marketplace (.napp) | `read_napp_entry(path, "SKILL.md")` or extracted directory | Signed archive |
| User (loose files) | Direct filesystem read | No signatures |

---

## 16. Legacy YAML Format

**Source:** `crates/tools/src/skills/loader.rs` — `load_yaml_skills()`

For backward compatibility, flat `.yaml` files in `user/skills/` are loaded as skills:

- File: `{name}.yaml` → enabled
- File: `{name}.yaml.disabled` → disabled
- Content becomes `skill.template`
- Name derived from filename (strip extension)
- Description set to `"YAML skill (legacy format)"`
- All other fields default (no triggers, no capabilities, priority 0)

Legacy YAML skills are only loaded if no SKILL.md-based skill with the same name exists.

---

## 17. Resource Files

### Structure

Skills can bundle resource files alongside SKILL.md:

```
my-skill/
├── SKILL.md              # Not listed as resource
├── manifest.json         # Not listed as resource
├── signatures.json       # Not listed as resource
├── scripts/
│   ├── run.py
│   └── utils.py
├── references/
│   └── style-guide.md
└── assets/
    └── template.xlsx
```

### Resource Discovery

```rust
pub fn list_resources(&self) -> Result<Vec<String>, String>
```

Recursively walks `base_dir`, collecting relative paths. Skips:
- Hidden files (starting with `.`)
- `SKILL.md` (case-insensitive)
- `manifest.json`
- `signatures.json`

### Resource Access

```rust
pub fn read_resource(&self, relative_path: &str) -> Result<Vec<u8>, String>
```

**Security:**
- Rejects paths containing `..` (path traversal)
- Guards against symlink escapes (resolved path must start with `base_dir`)
- Returns raw bytes (caller determines text vs binary)

### Extraction for Script Execution

When `ExecuteTool` runs a script, ALL resources are extracted to a temp directory preserving relative paths. This enables:
- `from scripts.utils import helper` (Python)
- `import ./lib/common.ts` (TypeScript)
- Asset file references via relative paths

---

## 18. Integration Points

### With Workflow System

- Activities reference skills by name in `activity.skills[]`
- SKILL.md content injected into activity prompts
- No global skill context bleed between activities
- Workflow `dependencies.skills[]` triggers cascade installation

### With Agent System

- Agents declare skill dependencies in `skills[]` (agent.json)
- Skills cascade-installed when agent is created
- Skills referenced by agent's workflows are also included in cascade

### With Agent Runner

- Skill triggers matched against user messages
- Matched skills provide context/instructions for the agent
- `tool_filter.rs` ensures `"skill"` tool is always available (core tool)

### With NeboLoop Marketplace

- Skills distributed as sealed `.napp` archives
- Install codes: `SKIL-XXXX-XXXX` (Crockford Base32)
- Three entry points: chat interception, SkillTool, REST API
- Cascade-installs dependencies recursively

### With Sandbox System

- Skill capabilities drive sandbox configuration
- `python`/`typescript` → needs sandbox
- `storage` → writable data dir
- `network` → allowed domains

### With Event System

- Skill installation broadcasts: `"code_processing"`, `"code_result"`, `"chat_complete"`
- Dependency cascade broadcasts: `"dep_installed"`, `"dep_pending"`, `"dep_failed"`, `"dep_cascade_complete"`

---

## 19. Constants & Defaults

```rust
// Skill validation
const MAX_NAME_LENGTH: usize = 64;
const MAX_DESCRIPTION_LENGTH: usize = 1024;
const MAX_COMPATIBILITY_LENGTH: usize = 500;

// Loader
const WATCHER_POLL_INTERVAL: Duration = Duration::from_secs(2);
const DEBOUNCE_DURATION: Duration = Duration::from_secs(1);

// ExecuteTool
const DEFAULT_TIMEOUT: u64 = 30;               // seconds
const RUNTIMES_DIR: &str = "/tmp/nebo-runtimes";

// Sandbox
const DENY_READ_PATHS: &[&str] = &["~/.ssh", "~/.gnupg", "~/.aws/credentials", "~/.config/gcloud"];
const PACKAGE_REGISTRIES: &[&str] = &["pypi.org", "files.pythonhosted.org", "registry.npmjs.org", "npm.pkg.github.com"];

// Default values
Skill::version = "1.0.0"
Skill::priority = 0
Skill::max_turns = 0                            // unlimited
Skill::source = SkillSource::User

// Crockford Base32 (code validation)
const CROCKFORD: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
```

---

## 20. Cross-Reference to Go Docs

| Rust (this doc) | Go Equivalent |
|---|---|
| `crates/tools/src/skills/skill.rs` | `internal/agent/skills/skill.go` |
| `crates/tools/src/skills/loader.rs` | `internal/agent/skills/loader.go` |
| `crates/tools/src/skill_tool.rs` | `internal/agent/tools/skill_domain.go` |
| `crates/tools/src/execute_tool.rs` | New in Rust (no Go equivalent) |
| `crates/tools/src/sandbox_policy.rs` | New in Rust (no Go equivalent) |
| `crates/server/src/handlers/skills.rs` | `internal/server/skill_routes.go` |
| `crates/server/src/codes.rs` | `internal/server/code_handler.go` |
| `crates/server/src/deps.rs` | New in Rust (no Go equivalent) |
| `crates/tools/src/lib.rs` (persist) | New in Rust (no Go equivalent) |

### Key File Index

| Component | Path |
|-----------|------|
| Skill struct & parsing | `crates/tools/src/skills/skill.rs` |
| Loader & hot-reload | `crates/tools/src/skills/loader.rs` |
| SkillTool (agent API) | `crates/tools/src/skill_tool.rs` |
| ExecuteTool (scripts) | `crates/tools/src/execute_tool.rs` |
| Sandbox policy | `crates/tools/src/sandbox_policy.rs` |
| REST handlers | `crates/server/src/handlers/skills.rs` |
| Code detection & dispatch | `crates/server/src/codes.rs` |
| Persistence | `crates/tools/src/lib.rs` (`persist_skill_from_api`) |
| Dependency cascade | `crates/server/src/deps.rs` |
| .napp reader | `crates/napp/src/reader.rs` |
| NeboLoop API client | `crates/comm/src/api.rs` |

**Canonical specification:**
- [platform-taxonomy.md](../.archive/platform-taxonomy.md) — Authoritative AGENT/WORK/SKILL hierarchy definition

---

*Last updated: 2026-03-25*
