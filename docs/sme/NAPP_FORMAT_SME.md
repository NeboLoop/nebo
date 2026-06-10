# NAPP Package Format -- SME Reference

> Subject Matter Expert document for the `.napp` binary package format used by
> Nebo to distribute skills, agents, plugins, tools, and workflows through the
> NeboAI marketplace.  Covers the wire format, cryptographic envelope,
> sealed-archive encryption, extraction pipeline, manifest schema, plugin and
> agent packaging, runtime execution, version resolution, hook system,
> supervision, and the end-to-end verification chain.

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Binary Envelope Format](#2-binary-envelope-format)
3. [Package Signing (ED25519)](#3-package-signing-ed25519)
4. [Sealed Archives (AES-256-GCM)](#4-sealed-archives-aes-256-gcm)
5. [Archive Internals (tar.gz)](#5-archive-internals-targz)
6. [Manifest (manifest.json)](#6-manifest-manifestjson)
7. [Agent Packaging](#7-agent-packaging)
8. [Plugin Packaging](#8-plugin-packaging)
9. [Skill Packaging](#9-skill-packaging)
10. [Package Reading API](#10-package-reading-api)
11. [Extraction Pipeline](#11-extraction-pipeline)
12. [Version Resolution](#12-version-resolution)
13. [Runtime Execution](#13-runtime-execution)
14. [Supervisor and Health](#14-supervisor-and-health)
15. [Hook System](#15-hook-system)
16. [Registry and Discovery](#16-registry-and-discovery)
17. [Agent Loader](#17-agent-loader)
18. [Plugin Store](#18-plugin-store)
19. [Sandbox and Environment](#19-sandbox-and-environment)
20. [Verification Chain](#20-verification-chain)
21. [Cross-System Interactions](#21-cross-system-interactions)
22. [Error Handling](#22-error-handling)
23. [Security Model](#23-security-model)
24. [Key Structs and Traits](#24-key-structs-and-traits)
25. [File Locations on Disk](#25-file-locations-on-disk)

---

## 1. Architecture Overview

```
 NeboAI Marketplace (server-side)
 +-------------------------------------------+
 |  Publish pipeline:                        |
 |    tar.gz(content)                        |
 |      -> SHA256(payload)                   |
 |      -> ED25519.sign(hash + payload)      |
 |      -> prepend NAPP envelope header      |
 |      -> optional: AES-256-GCM seal        |
 |      -> store as <version>.napp           |
 +-------------------------------------------+
              |
              | HTTPS download
              v
 Nebo Client (local machine)
 +-------------------------------------------+
 |                                           |
 |  +---------+    +----------+    +-------+ |
 |  | Registry |<-->| Runtime  |<-->|Process| |
 |  +---------+    +----------+    +-------+ |
 |       |              |                    |
 |       v              v                    |
 |  +---------+    +----------+              |
 |  |  Reader |    | Sandbox  |              |
 |  +---------+    +----------+              |
 |       |                                   |
 |       v                                   |
 |  +---------+    +----------+              |
 |  |Manifest |    |  Sealed  |              |
 |  +---------+    +----------+              |
 |       |              |                    |
 |       v              v                    |
 |  +---------+    +----------+              |
 |  | Signing |    | Version  |              |
 |  +---------+    +----------+              |
 |                                           |
 |  +-----------+  +----------+  +---------+ |
 |  |AgentLoader|  |PluginStore| |Supervisor| |
 |  +-----------+  +----------+  +---------+ |
 |                                           |
 |  +-----------+                            |
 |  |   Hooks   |                            |
 |  +-----------+                            |
 +-------------------------------------------+
```

### Crate: `nebo-napp`

Source: `crates/napp/src/`

Modules:

| Module           | Purpose                                               |
|------------------|-------------------------------------------------------|
| `napp.rs`        | Binary envelope unwrap, tar.gz extraction, validation |
| `sealed.rs`      | AES-256-GCM seal/unseal, HKDF key derivation          |
| `reader.rs`      | Random-access entry reading, partial extraction       |
| `signing.rs`     | ED25519 key management, signature verification        |
| `manifest.rs`    | `manifest.json` schema, qualified names, validation   |
| `agent.rs`       | `AGENT.md` + `agent.json` parsing, workflow bindings  |
| `plugin.rs`      | Plugin manifest, download, install, auth, store       |
| `runtime.rs`     | Process launch, socket wait, health check, PID files  |
| `sandbox.rs`     | Binary validation, environment sanitization           |
| `registry.rs`    | Tool discovery, launch, quarantine, sideload          |
| `agent_loader.rs`| Three-tier agent filesystem loader + hot-reload       |
| `version.rs`     | Semver resolution across installed .napp files        |
| `hooks.rs`       | Hook dispatcher, circuit breaker, plugin hook caller  |
| `supervisor.rs`  | Restart policy, exponential backoff, crash recovery   |
| `lib.rs`         | Error enum, install/quarantine event types, re-exports|

### Dependencies (Cargo.toml)

```
ed25519-dalek  2    (features = ["pkcs8"])   -- ED25519 signing
aes-gcm        0.10                          -- AES-256-GCM encryption
hkdf           0.12                          -- HKDF-SHA256 key derivation
sha2           (workspace)                   -- SHA-256 hashing
flate2         1                             -- gzip decompression
tar            0.4                           -- tar archive handling
base64         0.22                          -- base64 encode/decode
rand           (workspace)                   -- cryptographic randomness
semver         (workspace)                   -- version resolution
notify         7                             -- filesystem watcher
async-trait    0.1                           -- async trait objects
tempfile       3                             -- test helpers
libc           0.2                           -- Unix process management
```

---

## 2. Binary Envelope Format

Every `.napp` file is a binary envelope wrapping a tar.gz payload. The envelope
provides integrity verification and origin authentication before any content is
parsed.

### Wire Layout

```
Offset  Size   Field              Description
------  -----  -----------------  ------------------------------------------
0       4      Magic              b"NAPP" (0x4E 0x41 0x50 0x50)
4       1      Version            0x01 (envelope format version)
5       64     Signature          ED25519 signature (over hash + payload)
69      32     SHA256 Hash        SHA-256 digest of the payload
101     *      Payload            tar.gz archive (plain or AES-256-GCM sealed)
```

**Total header size: 101 bytes** (4 + 1 + 64 + 32)

### Constants (napp.rs)

```rust
const NAPP_MAGIC: &[u8; 4] = b"NAPP";
const NAPP_VERSION: u8 = 0x01;
const NAPP_HEADER_SIZE: usize = 4 + 1 + 64 + 32; // 101 bytes
```

### Unwrap Process

```
  .napp file bytes
  +------+---+----------+---------+------------------+
  | NAPP |v1 | sig(64B) | hash(32)| payload (tar.gz) |
  +------+---+----------+---------+------------------+
       |          |           |            |
       v          |           v            v
  1. Check magic  |    2. SHA256(payload)  |
     != "NAPP" -> |       != hash -> FAIL  |
     FAIL         |                        |
                  v                        |
           3. ED25519.verify(              |
                msg = hash || payload,     |
                sig = signature,           |
                key = NeboAI pubkey      |
              )                            |
              fail -> FAIL                 |
                                           v
                                    4. Return payload
                                       (tar.gz bytes)
```

### Key Functions

```rust
/// Verify and unwrap a .napp envelope, returning the inner tar.gz payload.
pub fn unwrap_napp(data: &[u8], public_key: &VerifyingKey) -> Result<Vec<u8>, NappError>

/// Convenience wrapper using the embedded NeboAI public key.
pub fn unwrap_napp_builtin(data: &[u8]) -> Result<Vec<u8>, NappError>

/// Full pipeline for sealed .napp files: unwrap envelope -> decrypt -> tar.gz.
pub fn unwrap_sealed_napp(data: &[u8], license_key: &[u8; 32]) -> Result<Vec<u8>, NappError>
```

---

## 3. Package Signing (ED25519)

All `.napp` packages are signed by NeboAI using ED25519 (via the
`ed25519-dalek` crate). The signing scheme provides two layers of verification:
envelope-level and content-level.

### Embedded Public Key

A 32-byte ED25519 public key is compiled into the Nebo binary at build time:

```rust
pub const NEBOAI_PUBLIC_KEY: &[u8; 32] = include_bytes!("../neboai_public_key.bin");
```

File: `crates/napp/neboai_public_key.bin` (32 bytes, raw ED25519 public key)

This enables offline verification without network access -- critical for
first-launch and air-gapped deployments.

### Remote Key Provider

For online verification, `SigningKeyProvider` caches a key fetched from NeboAI:

```rust
pub struct SigningKeyProvider {
    neboai_url: String,        // e.g., "https://api.neboai.com"
    key: RwLock<Option<CachedKey>>,
    ttl: Duration,               // 24 hours
}
```

Endpoint: `GET /api/v1/apps/signing-key`
Response: `{ "publicKey": "<base64-encoded ED25519 key>" }`

### Envelope Signature

The envelope signature covers the concatenation of the SHA256 hash and payload:

```
signed_message = SHA256_hash(32 bytes) || payload(N bytes)
signature = ED25519.sign(signing_key, signed_message)
```

This means verifying the signature implicitly verifies both the hash and the
payload content.

### Content-Level Signatures (signatures.json)

Inside the tar.gz, tool-type packages include a `signatures.json` file with
per-file signatures:

```json
{
    "manifest_signature": "<base64 ED25519 sig over manifest.json bytes>",
    "binary_hash": "<hex SHA256 of binary file>",
    "binary_signature": "<base64 ED25519 sig over binary bytes>"
}
```

Verification function:

```rust
pub fn verify_signatures(key: &VerifyingKey, app_dir: &Path) -> Result<(), NappError>
```

Steps:
1. Read `signatures.json` from the extracted directory
2. Load `manifest.json` bytes, verify `manifest_signature`
3. Find binary (`binary` or `app`), compute SHA256, compare to `binary_hash`
4. Verify `binary_signature` over the raw binary bytes

### Revocation Checker

```rust
pub struct RevocationChecker {
    neboai_url: String,
    cache: RwLock<Option<RevocationCache>>,
    ttl: Duration,  // 1 hour
}
```

Endpoint: `GET /api/v1/apps/revocations`
Response: `{ "revocations": ["app-id-1", "app-id-2", ...] }`

Behavior: **Fail open** -- if the network is unavailable, revocation check
returns `false` (not revoked). This prevents network outages from bricking
installed tools.

---

## 4. Sealed Archives (AES-256-GCM)

Sealed `.napp` files add an encryption layer on top of the signed envelope.
The payload is encrypted before being wrapped in the NAPP envelope, so the
envelope signature covers the ciphertext.

### Key Derivation

```
  Master Secret (from user license)
        |
        v
  HKDF-SHA256
    salt = artifact_id.as_bytes()
    info = b"neboai-license-v1"
        |
        v
  32-byte derived license key
```

```rust
pub fn derive_license_key(master_secret: &[u8], artifact_id: &str) -> [u8; 32]
```

Design decisions:
- Salt is artifact_id only -- license scope (user vs bot) is NOT part of
  derivation. Authorization is server-side.
- Same key works regardless of license holder -- sealed .napp files never need
  re-download on license transfer.
- Deterministic: same inputs always produce the same key.

### Encryption Format

```
Sealed payload layout:
+------------------+-------------------------------+
| 12-byte nonce    | AES-256-GCM ciphertext + tag  |
| (random)         | (N + 16 bytes)                |
+------------------+-------------------------------+
```

```rust
pub fn seal_payload(payload: &[u8], license_key: &[u8; 32]) -> Result<Vec<u8>, NappError>
pub fn unseal_payload(sealed: &[u8], license_key: &[u8; 32]) -> Result<Vec<u8>, NappError>
```

### Detecting Sealed vs Plain

```rust
pub fn is_sealed(payload: &[u8]) -> bool
```

Plain payloads are tar.gz and start with gzip magic bytes `0x1f 0x8b`.
Sealed payloads start with a 12-byte random nonce, which will not match.

### Full Sealed .napp Structure

```
+---------------------------------------------------------------+
| NAPP envelope header (101 bytes)                              |
|   Magic: "NAPP"                                               |
|   Version: 0x01                                               |
|   Signature: ED25519(hash || sealed_payload)                  |
|   SHA256: hash(sealed_payload)                                |
+---------------------------------------------------------------+
| Sealed payload:                                               |
|   +-------------------+------------------------------------+  |
|   | Nonce (12 bytes)  | AES-256-GCM(tar.gz) + Tag (16B)   |  |
|   +-------------------+------------------------------------+  |
|                                                               |
|   When decrypted, yields:                                     |
|     tar.gz archive containing:                                |
|       manifest.json                                           |
|       AGENT.md / SKILL.md / PLUGIN.md                         |
|       agent.json / plugin.json                                |
|       binary / app                                            |
|       signatures.json                                         |
|       ui/ (directory)                                         |
|       skills/ (directory)                                     |
+---------------------------------------------------------------+
```

---

## 5. Archive Internals (tar.gz)

The inner payload (after envelope unwrap and optional decryption) is a standard
tar.gz archive. The extraction pipeline enforces strict allowlists and security
checks.

### Allowed Files

```rust
const ALLOWED_FILES: &[&str] = &[
    "manifest.json",
    "binary",
    "app",
    "signatures.json",
    "TOOL.md",    "tool.md",
    "WORKFLOW.md","workflow.md",
    "workflow.json",
    "AGENT.md",   "agent.md",
    "agent.json",
    "SKILL.md",   "skill.md",
    "PLUGIN.md",  "plugin.md",
    "plugin.json",
];
```

### Directory Prefixes

- `ui/` -- Static UI assets (HTML, CSS, JS) for app-type agents
- `skills/` -- Skill markdown files (only `SKILL.md` files allowed)

### Size Limits

```rust
const MAX_BINARY_SIZE: u64   = 500 * 1024 * 1024; // 500 MB
const MAX_UI_FILE_SIZE: u64  =   5 * 1024 * 1024; //   5 MB
const MAX_METADATA_SIZE: u64 =   1 * 1024 * 1024; //   1 MB
```

### Binary Validation

Binaries must be native executables. The extraction pipeline reads the first 4
bytes (magic) and validates the format:

| Format       | Magic Bytes                                |
|--------------|--------------------------------------------|
| ELF (Linux)  | `7F 45 4C 46`                              |
| Mach-O 32    | `FE ED FA CE`                              |
| Mach-O 64    | `FE ED FA CF`                              |
| Mach-O 32 sw | `CE FA ED FE`                              |
| Mach-O 64 sw | `CF FA ED FE`                              |
| Universal    | `CA FE BA BE`                              |
| PE (Windows) | `4D 5A`                                    |

Rejected: shebang scripts (`#!`), unrecognized formats.

### Security Checks During Extraction

1. **Path traversal**: Reject entries containing `..` or starting with `/`
2. **Symlinks/hardlinks**: Reject all symlink and hardlink entries
3. **Allowlist**: Reject files not in the allowlist (except `ui/`, `skills/`)
4. **Size enforcement**: Double-checked -- both header size and actual read size
5. **Canonicalization**: Target path verified to remain within destination
6. **Manifest required**: Extraction fails if no `manifest.json` found

---

## 6. Manifest (manifest.json)

Universal envelope for all artifact types. Every `.napp` archive contains a
`manifest.json` at the root.

### Schema

```rust
pub struct Manifest {
    pub id: String,                         // Unique identifier
    pub name: String,                       // Display name or @org/type/name
    pub version: String,                    // Semver string
    pub artifact_type: String,              // "skill", "tool", "workflow", "agent", "app"
    pub description: String,
    pub author: String,
    pub code: String,                       // Marketplace code (assigned on publish)
    pub tags: Vec<String>,
    pub runtime: String,                    // "local" (default)
    pub protocol: String,                   // "grpc" (default)
    pub signature: Option<ManifestSignature>,
    pub startup_timeout: u32,               // 0-120 seconds (default 10)
    pub provides: Vec<String>,              // Capabilities: "gateway", "vision", etc.
    pub permissions: Vec<String>,           // "network:*", "tool:web", etc.
    pub overrides: Vec<String>,             // Hook overrides (require hook: permission)
    pub oauth: Vec<OAuthRequirement>,
    pub implements: Vec<String>,            // Abstract tool IDs this tool provides
    pub window: Option<AppWindowConfig>,    // Window config for app-type agents
}
```

### Qualified Names

Artifacts use a `@org/type/name` naming scheme:

```rust
pub struct QualifiedName {
    pub org: String,            // e.g., "acme"
    pub artifact_type: String,  // "skills", "workflows", "agents"
    pub artifact_name: String,  // e.g., "crm-lookup"
}
```

Valid types: `skills`, `workflows`, `agents`

Example: `@nebo/skills/briefing-writer`

### Validation Rules

- `id` required (unless name starts with `@`)
- `name` required, non-empty
- `version` required, non-empty
- Capabilities must be in `VALID_CAPABILITIES` list
- Permissions must start with valid prefix (24 valid prefixes)
- Overrides require corresponding `hook:` permission
- `startup_timeout` capped at 120 seconds

### Permission Prefixes

```
network:  filesystem:  settings:  capability:  memory:  session:
context:  tool:        shell:     subagent:    lane:    channel:
comm:     notification: embedding: skill:      advisor: model:
mcp:      database:    storage:   schedule:    voice:   browser:
oauth:    user:        hook:
```

---

## 7. Agent Packaging

Agents are AI personas with workflow definitions. They consist of two primary
files and optional supporting resources.

### File Layout (Agent .napp)

```
agent.napp (tar.gz)
|-- manifest.json       Required: identity + version + artifact_type:"agent"
|-- AGENT.md            Required: persona description (pure prose)
|-- agent.json          Optional: workflow bindings, triggers, skills, tools
|-- ui/                 Optional: static UI assets (app-type agents only)
|   |-- index.html
|   |-- style.css
|   +-- ...
|-- bin/                Optional: compiled sidecar binary (app-type agents)
|   +-- sidecar-name
|-- skills/             Optional: embedded skill definitions
|   +-- SKILL.md
+-- signatures.json     Optional: content signatures
```

### AGENT.md

Pure prose -- the agent's job description. No frontmatter required for new
agents. Legacy format with YAML frontmatter is supported for backward
compatibility.

```rust
pub fn parse_agent(content: &str) -> Result<AgentDef, NappError>
```

New format (pure prose):
```markdown
# Chief of Staff

You manage the executive's daily rhythm. Your primary responsibilities...
```

Legacy format (frontmatter):
```markdown
---
id: sales-sdr
name: Sales SDR
---
# Sales SDR

Body text.
```

### agent.json

Carries operational structure: workflow definitions, triggers, skill
dependencies, sidecar tools, pricing, and memory configuration.

```rust
pub struct AgentConfig {
    pub workflows: HashMap<String, WorkflowBinding>,
    pub skills: Vec<String>,              // Qualified skill refs
    pub requires: AgentRequires,          // Hard dependencies (plugins)
    pub pricing: Option<AgentPricing>,
    pub defaults: Option<AgentDefaults>,
    pub inputs: Vec<AgentInputField>,     // Dynamic setup form
    pub tools: Vec<AgentToolDef>,         // Sidecar HTTP tools
    pub scopes: HashMap<String, ToolScope>, // SDK embed tool scopes
    pub memory: MemoryConfig,             // Memory inheritance + isolation
}
```

### Workflow Bindings

Each workflow has a trigger and optional inline activities:

```rust
pub struct WorkflowBinding {
    pub trigger: AgentTrigger,                // Schedule, Heartbeat, Event, Watch, Manual
    pub description: String,
    pub inputs: HashMap<String, serde_json::Value>,
    pub activities: Vec<AgentActivity>,       // Inline procedure
    pub connections: Vec<WorkflowConnection>, // Visual graph edges
    pub budget: AgentBudget,
    pub emit: Option<String>,                 // Event to emit on completion
}
```

### Trigger Types

```rust
pub enum AgentTrigger {
    Schedule { cron: String, schedule: Option<String> },
    Heartbeat { interval: String, window: Option<String> },
    Event { sources: Vec<String> },
    Watch { plugin: String, command: String, event: Option<String>, restart_delay_secs: u64 },
    Manual,
}
```

### Sidecar Tool Definitions

Agents can declare HTTP tools routed to a sidecar process:

```rust
pub struct AgentToolDef {
    pub name: String,            // "list_projects"
    pub description: String,
    pub method: String,          // HTTP method: GET, POST, PUT, DELETE
    pub path: String,            // "/projects/{id}"
    pub input_schema: Option<serde_json::Value>,  // JSON Schema
}
```

### Memory Configuration

```rust
pub struct MemoryConfig {
    pub inherit_user: bool,       // Read-only inheritance from user's main memories
    pub context_isolated: bool,   // Per-contextId memory isolation
}
```

Three-tier memory hierarchy:
- Layer 1 (User): `user_id = "user123"`
- Layer 2 (Agent): `user_id = "user123:agent:brief"`
- Layer 3 (Context): `user_id = "user123:agent:brief:ctx:doc-123"`

---

## 8. Plugin Packaging

Plugins are managed binaries shared across skills. They are downloaded once and
resolved by slug + semver range.

### File Layout (Plugin .napp)

```
plugin.napp (tar.gz)
|-- manifest.json       Optional: universal envelope
|-- plugin.json         Required: plugin-specific manifest
|-- PLUGIN.md           Optional: documentation
|-- binary              Required: compiled native binary (or named binary)
|-- signatures.json     Optional: content signatures
|-- skills/             Optional: plugin skill documentation
|   |-- gmail/
|   |   +-- SKILL.md
|   |-- calendar/
|   |   +-- SKILL.md
|   +-- ...
+-- ui/                 Optional: UI assets
```

### Plugin Manifest (plugin.json)

```rust
pub struct PluginManifest {
    pub id: String,                                 // NeboAI artifact ID
    pub slug: String,                               // URL-safe slug (e.g., "gws")
    pub name: String,                               // Display name
    pub version: String,                            // Semver
    pub description: String,
    pub author: String,
    pub platforms: HashMap<String, PlatformBinary>,  // Per-platform binaries
    pub signing_key_id: String,
    pub env_var: String,                            // Custom env var override
    pub auth: Option<PluginAuth>,                   // Auth configuration
    pub events: Option<Vec<PluginEventDef>>,        // Watch events
    pub dependencies: Vec<PluginDependency>,        // Plugin-to-plugin deps
    pub capabilities: Option<PluginCapabilities>,   // Structured capabilities
    pub permissions: Option<PluginPermissions>,      // Sandbox permissions
    pub category: String,                           // Discovery category
    pub triggers: Vec<String>,                      // Search keywords
}
```

### Platform Binary Entry

```rust
pub struct PlatformBinary {
    pub binary_name: String,     // "gws" or "gws.exe"
    pub sha256: String,          // Hex SHA256 of binary
    pub signature: String,       // Base64 ED25519 signature
    pub size: u64,               // File size in bytes
    pub download_url: String,    // Download URL
}
```

Platform keys: `"darwin-arm64"`, `"darwin-x86_64"`, `"linux-x86_64"`,
`"windows-x86_64"`, etc.

### Plugin Capabilities

Plugins declare structured capabilities that Nebo registers at runtime:

```rust
pub struct PluginCapabilities {
    pub tools: Vec<PluginToolDef>,       // CLI-backed tools
    pub hooks: Vec<PluginHookDef>,       // Lifecycle hooks
    pub commands: Vec<PluginCommandDef>, // Slash commands
    pub routes: Vec<PluginRouteDef>,     // HTTP routes (proxied)
    pub providers: Vec<PluginProviderDef>, // Model/speech/image providers
    pub config_schema: Vec<PluginConfigField>, // Settings form
}
```

### Plugin Auth

```rust
pub struct PluginAuth {
    pub auth_type: String,                // "oauth_cli", "env"
    pub env: HashMap<String, String>,     // Env vars injected during auth
    pub commands: PluginAuthCommands,      // login, status, logout
    pub label: String,                    // "Google Account"
    pub description: String,
}

pub struct PluginAuthCommands {
    pub login: String,                    // "auth login"
    pub status: Option<String>,           // "auth status" (JSON output)
    pub logout: Option<String>,           // "auth logout"
}
```

### Plugin Validation Rules

- Slug: lowercase alphanumeric + hyphens, max 64 chars, no leading/trailing/consecutive hyphens
- Version: valid semver
- At least one platform entry
- Binary names: no path separators, no `..`, non-empty
- Auth: login command required unless auth_type is "env"
- Events: non-empty name and command, no path separators in names

---

## 9. Skill Packaging

Skills are the simplest artifact type -- markdown documentation files.

### File Layout (Skill .napp)

```
skill.napp (tar.gz)
|-- manifest.json       Required: identity
|-- SKILL.md            Required: skill documentation with YAML frontmatter
+-- signatures.json     Optional
```

Skills can also be nested inside plugin and agent .napp archives in the
`skills/` directory.

---

## 10. Package Reading API

The `reader.rs` module provides random-access reading of `.napp` archives
without requiring full extraction. This is critical for sealed archives where
plaintext should never touch disk.

### Plain .napp Readers

```rust
/// Read a single entry by name.
pub fn read_napp_entry(napp_path: &Path, entry_name: &str) -> Result<Vec<u8>, NappError>

/// Read a single entry as UTF-8 string.
pub fn read_napp_entry_string(napp_path: &Path, entry_name: &str) -> Result<String, NappError>

/// List all entry names.
pub fn list_napp_entries(napp_path: &Path) -> Result<Vec<String>, NappError>

/// Extract a single entry to a destination path.
pub fn extract_napp_entry(napp_path: &Path, entry_name: &str, dest: &Path) -> Result<(), NappError>

/// Extract all entries matching a prefix (e.g., "ui/").
pub fn extract_napp_prefix(napp_path: &Path, prefix: &str, dest_dir: &Path) -> Result<Vec<String>, NappError>

/// Extract all entries preserving structure.
pub fn extract_all(napp_path: &Path, dest_dir: &Path) -> Result<Vec<String>, NappError>

/// Extract to sibling directory (strip .napp extension).
pub fn extract_napp_alongside(napp_path: &Path) -> Result<PathBuf, NappError>
```

### Sealed .napp Readers

Sealed readers decrypt in memory and never write plaintext to disk:

```rust
/// Read a single entry from a sealed archive.
pub fn read_sealed_napp_entry(
    napp_path: &Path, entry_name: &str, license_key: &[u8; 32]
) -> Result<Vec<u8>, NappError>

/// Read as UTF-8 string from sealed archive.
pub fn read_sealed_napp_entry_string(
    napp_path: &Path, entry_name: &str, license_key: &[u8; 32]
) -> Result<String, NappError>

/// List entries in sealed archive.
pub fn list_sealed_napp_entries(
    napp_path: &Path, license_key: &[u8; 32]
) -> Result<Vec<String>, NappError>

/// Partial extraction: executables + metadata only, IP stays sealed.
pub fn partial_extract_sealed_napp(
    napp_path: &Path, license_key: &[u8; 32]
) -> Result<Option<PathBuf>, NappError>
```

### Partial Extraction Rules

Only these entries are extracted from sealed archives:

```rust
fn is_partial_extract_entry(name: &str) -> bool {
    name == "binary" || name == "app"
        || name.starts_with("scripts/") || name.starts_with("bin/")
        || name == "manifest.json" || name == "plugin.json"
        || name == "signatures.json"
}
```

Intellectual property (SKILL.md, AGENT.md, references/, assets/) stays inside
the sealed archive and is read in-memory at runtime.

### Directory Walker

```rust
/// Walk a directory tree, calling `cb` on dirs containing `marker_file`.
pub fn walk_for_marker(dir: &Path, marker_file: &str, cb: &mut dyn FnMut(&Path))
```

Single-pass per directory (one `read_dir()` call checks marker presence and
discovers subdirectories). Does not recurse into marker directories.

### Plugin Identity Reader

```rust
/// Read plugin slug + version from plugin.json in a tar.gz payload.
pub fn read_plugin_identity_from_tar_gz(payload: &[u8]) -> Result<(String, String), NappError>
```

Used during bundled plugin seeding to identify plugins before full extraction.

---

## 11. Extraction Pipeline

### Full Extraction (extract_napp, napp.rs)

```
  .napp file on disk
        |
        v
  Open file -> GzDecoder -> tar::Archive
        |
        v
  For each tar entry:
    1. Read path, normalize (strip "./")
    2. Security: reject ".." and leading "/"
    3. Security: reject symlinks and hardlinks
    4. Classify: is_ui? is_skill? is_binary? is_allowed?
    5. Reject unknown files
    6. Check size against type-specific limit
    7. Build target path, verify within dest_dir
    8. Create parent directories
    9. Canonicalize and verify no path escape
   10. Read content, verify actual size
   11. Write to disk
   12. If binary: validate_binary_format + chmod 0o755
   13. Track manifest.json presence
        |
        v
  Verify manifest.json found
        |
        v
  Load + validate manifest
        |
        v
  Return Manifest
```

### Sealed Partial Extraction

```
  sealed .napp file
        |
        v
  Read entire file into memory
        |
        v
  unwrap_napp_builtin():
    verify magic, version, SHA256, ED25519
        |
        v
  unseal_payload():
    AES-256-GCM decrypt with license key
        |
        v
  Iterate tar entries in memory:
    - Extract ONLY executables + metadata
    - Skip SKILL.md, AGENT.md, assets (IP protection)
    - Set +x on binary, app, scripts/, bin/
        |
        v
  Return extraction directory path
  (or None if nothing to extract)
```

---

## 12. Version Resolution

The `version.rs` module handles semver-based resolution across installed
`.napp` files. Archives are named `<version>.napp` within qualified-name
directories.

### Directory Structure

```
<base_dir>/
  @acme/skills/sales-qualification/
    1.0.0.napp
    1.1.0.napp
    2.0.0.napp
  @nebo/skills/briefing-writer/
    1.0.0.napp
```

### Resolution Functions

```rust
/// Resolve best matching .napp for a qualified name and semver range.
/// Returns the path to the highest version satisfying the range.
pub fn resolve_version(
    base_dir: &Path, qualified_name: &str, range: &str
) -> Result<PathBuf, NappError>

/// List all installed versions, sorted newest first.
pub fn list_versions(
    base_dir: &Path, qualified_name: &str
) -> Result<Vec<(semver::Version, PathBuf)>, NappError>

/// Get the latest installed version.
pub fn latest_version(base_dir: &Path, qualified_name: &str) -> Result<PathBuf, NappError>
```

### Range Handling

- Empty string or `"*"` -> return highest version
- `"^1.0.0"` -> `>=1.0.0, <2.0.0`
- `"~1.0.0"` -> `>=1.0.0, <1.1.0`
- `"=1.0.0"` -> exact match

Non-semver filenames are silently skipped.

---

## 13. Runtime Execution

The `runtime.rs` module manages the lifecycle of tool sidecar processes.

### Process Structure

```rust
pub struct Process {
    pub tool_id: String,
    pub manifest: Manifest,
    pub pid: u32,
    pub sock_path: PathBuf,       // Unix domain socket
    pub binary_path: PathBuf,
    binary_mtime: SystemTime,     // For change detection
    child: tokio::process::Child,
}
```

### Launch Sequence

```
  tool_dir/
    manifest.json
    binary (or app, or bin/*, or sidecar/target/release/*)
        |
        v
  1. Load + validate manifest
  2. Find binary (search order: binary, app, tmp/*, bin/*, sidecar/target/release/*)
  3. Snapshot binary mtime for change detection
  4. Validate binary (sandbox::validate_binary)
  5. Determine socket path: tool_dir/<manifest.id>.sock
  6. Clean up stale socket
  7. Create data/ directory
  8. Build sanitized environment (env_clear + allowlisted vars)
  9. Configure stdout/stderr -> data/sidecar.log
 10. Set process group isolation (Unix: setpgid)
 11. Spawn process
 12. Write PID file
 13. Wait for socket (exponential backoff, configurable timeout)
 14. Health check (Unix socket connect, best-effort)
 15. Set socket permissions (0o600)
 16. Return Process
```

### Binary Search Order

```rust
fn find_binary(&self, tool_dir: &Path) -> Result<PathBuf, NappError>
```

1. `tool_dir/binary`
2. `tool_dir/app`
3. First file in `tool_dir/tmp/`
4. First file in `tool_dir/bin/`
5. First executable file in `tool_dir/sidecar/target/release/`

### Process Lifecycle

```rust
impl Process {
    pub fn grpc_endpoint(&self) -> String    // "unix:///path/to.sock"
    pub fn binary_changed(&self) -> bool     // Detects rebuild (follows symlinks)
    pub fn is_alive(&self) -> bool           // kill(pid, 0) check
    pub async fn stop(&mut self)             // SIGTERM -> 2s wait -> SIGKILL
}
```

Graceful shutdown: SIGTERM to process group -> wait 2 seconds -> force kill.

### Stale Process Cleanup

```rust
pub fn cleanup_stale(&self, tool_dir: &Path)
```

Finds `.pid` files, checks if process is alive, sends SIGTERM, removes PID files.

---

## 14. Supervisor and Health

The `supervisor.rs` module implements restart policies with exponential backoff.

### Restart Policy

```
  Max 5 restarts per hour
  Exponential backoff: 10s -> 20s -> 40s -> 80s -> 160s (cap 5 min)
  Window resets after 1 hour
```

```rust
pub struct Supervisor {
    states: RwLock<HashMap<String, RestartState>>,
    check_interval: Duration,  // 15 seconds
}
```

### API

```rust
pub async fn watch(&self, app_id: &str)           // Register for supervision
pub async fn unwatch(&self, app_id: &str)          // Unregister
pub async fn should_restart(&self, app_id: &str) -> bool  // Check if restart allowed
pub async fn record_restart(&self, app_id: &str)   // Record restart event
pub async fn restart_count(&self, app_id: &str) -> u32
pub fn check_interval(&self) -> Duration           // 15 seconds
```

### Integration with AppLifecycle

The `app_lifecycle.rs` module in `crates/server/` spawns a health-checker
tokio task per app agent that:

1. Runs every `supervisor.check_interval()` (15s)
2. Checks if binary changed on disk -> hot-restart (no backoff)
3. Checks if process died -> crash restart with backoff
4. Unregisters/re-registers sidecar tools on restart
5. Broadcasts WebSocket events: `app_started`, `app_stopped`, `app_crashed`,
   `app_restarted`

---

## 15. Hook System

The `hooks.rs` module implements a plugin hook dispatcher with circuit
breaker pattern.

### Valid Hooks

```rust
pub const VALID_HOOKS: &[&str] = &[
    "tool.pre_execute",
    "tool.post_execute",
    "message.pre_send",
    "message.post_receive",
    "memory.pre_store",
    "memory.pre_recall",
    "session.message_append",
    "prompt.system_sections",
    "steering.generate",
    "response.stream",
    "agent.turn",
    "agent.should_continue",
];
```

### Hook Types

- **Filter**: Chain payload through subscribers in priority order. Can modify
  payload. If `handled = true`, chain stops. On error, original payload preserved.
- **Action**: Fire-and-forget to all subscribers. Errors logged, not propagated.

### HookDispatcher

```rust
pub struct HookDispatcher {
    hooks: RwLock<HashMap<String, Vec<HookSubscription>>>,
    timeout: Duration,       // 500ms default
    max_failures: u32,       // 3 failures -> disable
}
```

### Circuit Breaker

- 3 consecutive failures -> hook disabled
- 5-minute cooldown -> auto re-enable
- Success resets failure counter

### Plugin Hook Caller

Subprocess-backed caller that spawns the plugin binary with hook subcommand:

```rust
pub struct PluginHookCaller {
    binary_path: PathBuf,
    command: String,          // e.g., "hook pre_execute"
    plugin_slug: String,
    timeout: Duration,
}
```

Protocol:
1. Spawn plugin binary with hook args
2. Write JSON payload to stdin
3. Read JSON from stdout
4. Exit 0 = success

Filter response format: `{ "payload": ..., "handled": bool }`

### Registration

```rust
pub fn register_plugin_hooks(
    manifest: &PluginManifest, binary_path: &Path, dispatcher: &HookDispatcher
) -> usize
```

---

## 16. Registry and Discovery

The `registry.rs` module manages the full lifecycle of tool processes.

### Two-Directory Model

```
<data_dir>/
  nebo/tools/           -- Marketplace tools (sealed .napp + extracted binary)
  user/tools/           -- User/sideloaded tools (loose files, symlinks)
```

### RegistryConfig

```rust
pub struct RegistryConfig {
    pub installed_tools_dir: PathBuf,  // nebo/tools/
    pub user_tools_dir: PathBuf,       // user/tools/
    pub neboai_url: Option<String>,
}
```

### Discovery Flow

```
  discover_and_launch()
        |
        +-- discover_installed_tools()
        |     Walk nebo/tools/ for .napp files
        |     For each .napp:
        |       1. Read manifest from archive
        |       2. Validate manifest
        |       3. Check binary exists in sibling dir
        |       4. Verify binary integrity (SHA256)
        |       5. Check revocation list
        |       6. Verify ED25519 signatures
        |       7. Launch via Runtime
        |
        +-- discover_user_tools()
              Walk user/tools/ directories
              For each tool_dir:
                1. Skip quarantined (.quarantined marker)
                2. Load manifest.json
                3. Validate manifest
                4. Launch via Runtime (no signature check)
```

### Install from URL

```rust
pub async fn install_from_url(&self, url: &str) -> Result<String, NappError>
```

1. Download to temp file (500MB limit)
2. Read manifest from archive
3. Build qualified path: `nebo/tools/<tool_id>/<version>.napp`
4. Extract binary and `ui/` to version directory
5. Write `manifest.json` to version dir
6. Launch process

### Sideloading

```rust
pub async fn sideload(&self, project_dir: &Path) -> Result<String, NappError>
pub async fn unsideload(&self, tool_id: &str) -> Result<(), NappError>
```

Creates a symlink from `user/tools/<tool_id>` to the developer's project
directory. No signature verification -- for development only.

### Quarantine

```rust
async fn quarantine(&self, tool_id: &str, tool_dir: &Path, reason: &str)
```

1. Stop process
2. Remove binary (preserve `data/` and logs)
3. Create `.quarantined` marker file with reason
4. Emit `QuarantineEvent` callback

### Install Events

```rust
pub async fn handle_install_event(&self, event: InstallEvent) -> Result<(), NappError>
```

Handles MQTT/WebSocket events from NeboAI:
- `tool_installed` -> download and install
- `tool_uninstalled` -> stop and remove
- `tool_revoked` -> quarantine

---

## 17. Agent Loader

The `agent_loader.rs` module implements a three-tier filesystem scanner for
agent content with hot-reload support.

### Loading Priority (lowest to highest)

```
  1. Embedded bundled agents   -- compiled into binary (include_str!)
  2. Installed agents          -- nebo/agents/ (marketplace .napp archives)
  3. User agents               -- user/agents/ (loose files, highest priority)
```

Higher priority overrides lower by name (case-insensitive).

### LoadedAgent

```rust
pub struct LoadedAgent {
    pub agent_def: AgentDef,                  // AGENT.md content
    pub config: Option<AgentConfig>,          // agent.json
    pub source: AgentSource,                  // Installed or User
    pub napp_path: Option<PathBuf>,           // Path to .napp archive
    pub source_path: PathBuf,                 // Directory or file path
    pub version: Option<String>,
    pub agent_md: String,                     // Raw AGENT.md (for DB sync)
    pub frontmatter: String,                  // Raw agent.json (for DB sync)
    pub description: String,
    pub id: Option<String>,                   // NeboAI artifact UUID
    pub is_app: bool,                         // artifact_type == "app"
    pub app_ui_path: Option<PathBuf>,         // ui/ directory
    pub app_binary_path: Option<PathBuf>,     // sidecar binary
    pub app_window_config: Option<AppWindowConfig>,
}
```

### Hot-Reload (Filesystem Watcher)

```rust
pub fn watch(&self) -> (JoinHandle<()>, Receiver<AgentFsEvent>)
```

Uses the `notify` crate (v7) to watch both `installed_dir` and `user_dir`.
Debounces events to 1 second. Emits diff events:

```rust
pub enum AgentFsEvent {
    Added(LoadedAgent),
    Changed(LoadedAgent),     // agent_md, frontmatter, or theme_css differ
    Removed { name_key: String, agent: LoadedAgent },
}
```

Relevant file changes: `AGENT.md`, `agent.json`, `manifest.json`, `*.napp`, new directories.

### Sealed Agent Loading

```rust
pub fn scan_sealed_agents(
    dir: &Path, license_keys: &HashMap<String, [u8; 32]>
) -> Vec<LoadedAgent>
```

For sealed `.napp` archives:
1. Find `.napp` files without sibling AGENT.md directories
2. Read `artifact_id` from partially-extracted `manifest.json`
3. Look up license key by artifact_id
4. Read AGENT.md, agent.json from encrypted archive in memory (plaintext never on disk)

---

## 18. Plugin Store

The `plugin.rs` module's `PluginStore` manages the full plugin lifecycle.

### Directory Structure

```
<data_dir>/
  nebo/plugins/         -- Marketplace plugin downloads
    gws/
      1.2.0/
        gws (binary)
        plugin.json
        skills/
          gmail/SKILL.md
          calendar/SKILL.md
      1.2.0.napp        -- Sealed archive (optional)
  user/plugins/         -- User-provided (overrides installed)
    my-plugin/
      plugin.json
      my-plugin (binary)
  plugins/data/         -- Persistent data directory (survives updates)
    gws/
      oauth-tokens.json
```

### Resolution

```rust
pub fn resolve(&self, slug: &str, version_range: &str) -> Option<PathBuf>
```

1. Check `user_dir` first (user override)
2. Check `installed_dir` (marketplace)
3. For each directory: scan version subdirs, match semver range, return
   highest matching binary path
4. Also checks flat layout (plugin.json at slug root for dev repos)

### Download and Install

```rust
pub async fn ensure<F, Fut>(
    &self, slug: &str, version_range: &str, download_fn: F
) -> Result<PathBuf, NappError>
```

1. Fast path: already installed locally -> return path
2. Dedup concurrent downloads via `downloading` mutex
3. Call `download_fn` to get manifest + binary bytes from NeboAI
4. Validate manifest
5. Verify SHA256 hash against manifest entry
6. Verify ED25519 signature (if signing key available)
7. Write binary to `installed_dir/slug/version/binary_name`
8. Set executable permissions
9. Write `plugin.json` for future reference
10. Cache manifest in memory

### .napp-Based Plugin Install

```rust
pub async fn install_from_napp(
    &self, slug: &str, version: &str, napp_data: &[u8]
) -> Result<PathBuf, NappError>
```

1. Store .napp at `installed_dir/slug/version.napp`
2. Extract alongside to `installed_dir/slug/version/`
3. Same pattern as agent install

### Auth Cache

```rust
pub async fn refresh_auth_cache(&self)        // Startup: parallel auth status checks
pub async fn update_auth_status(&self, slug: &str)  // After login/logout
pub async fn check_auth_lazy(&self, slug: &str) -> bool  // First-access cached
pub async fn plugins_needing_auth(&self) -> Vec<(String, PluginAuth)>
```

### Diagnostics

```rust
pub fn record_diagnostic(&self, slug: &str, level: &str, phase: &str, message: &str)
pub fn get_diagnostics(&self, slug: &str) -> Vec<PluginDiagnostic>
```

In-memory diagnostic log capped at 1000 entries (drains oldest 100 on overflow).

---

## 19. Sandbox and Environment

The `sandbox.rs` module enforces isolation for tool processes.

### Environment Sanitization

```rust
pub fn sanitize_env(
    app_id: &str, app_name: &str, app_version: &str,
    app_dir: &str, sock_path: &str, data_dir: &str
) -> Vec<(String, String)>
```

Process environment is `env_clear()`'d, then populated with:

**Nebo-specific variables:**

| Variable           | Value            |
|--------------------|------------------|
| `NEBO_APP_ID`      | manifest id      |
| `NEBO_APP_NAME`    | manifest name    |
| `NEBO_APP_VERSION` | manifest version |
| `NEBO_APP_DIR`     | tool directory   |
| `NEBO_APP_SOCK`    | socket path      |
| `NEBO_DATA_DIR`    | data directory   |

**Allowlisted system variables:**
`PATH`, `HOME`, `TMPDIR`, `LANG`, `LC_ALL`, `TZ`

**Blocked variables (never passed through):**
`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GOOGLE_API_KEY`, `JWT_SECRET`,
`DATABASE_URL`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `GITHUB_TOKEN`,
`STRIPE_SECRET_KEY`

### Binary Validation

```rust
pub fn validate_binary(path: &Path, max_size: u64) -> Result<(), NappError>
```

Checks:
1. Not a symlink
2. Regular file
3. Within size limit
4. Executable permission set (Unix)
5. Native binary format (ELF, Mach-O, PE) -- rejects shebang scripts

---

## 20. Verification Chain

End-to-end verification from download to execution:

```
  Download .napp from NeboAI
  +-----------------------------------------------------+
  |                                                     |
  v                                                     |
  1. ENVELOPE VERIFICATION (napp.rs::unwrap_napp)       |
     a. Check magic bytes == "NAPP"                     |
     b. Check version == 0x01                           |
     c. SHA256(payload) == stored hash                  |
     d. ED25519.verify(hash||payload, sig, pubkey)      |
                                                        |
  2. OPTIONAL DECRYPTION (sealed.rs::unseal_payload)    |
     a. HKDF-SHA256(master_secret, artifact_id) -> key  |
     b. AES-256-GCM decrypt(nonce, ciphertext, key)     |
                                                        |
  3. ARCHIVE EXTRACTION (napp.rs::extract_napp)         |
     a. Path traversal prevention                       |
     b. Symlink/hardlink rejection                      |
     c. File allowlist enforcement                      |
     d. Size limit enforcement                          |
     e. Binary format validation                        |
     f. Manifest load and validation                    |
                                                        |
  4. SIGNATURE VERIFICATION (signing.rs::verify_sigs)   |
     a. Verify manifest_signature over manifest.json    |
     b. SHA256(binary) == binary_hash                   |
     c. Verify binary_signature over binary bytes       |
                                                        |
  5. REVOCATION CHECK (signing.rs::RevocationChecker)   |
     a. GET /api/v1/apps/revocations                    |
     b. Cache for 1 hour                                |
     c. Fail open if network unavailable                |
                                                        |
  6. RUNTIME VALIDATION (sandbox.rs::validate_binary)   |
     a. Not a symlink                                   |
     b. Regular file, within size limit                 |
     c. Executable permission                           |
     d. Native binary format (ELF/Mach-O/PE)           |
                                                        |
  7. BINARY INTEGRITY (registry.rs::verify_installed)   |
     a. Re-read manifest from sealed archive            |
     b. SHA256(extracted binary) == manifest hash       |
     c. On mismatch: quarantine tool                    |
  +-----------------------------------------------------+
```

---

## 21. Cross-System Interactions

### Marketplace -> Client

```
  NeboAI publishes .napp
        |
        v
  InstallEvent (via MQTT/WebSocket)
    { type: "tool_installed", tool_id, payload: { download_url } }
        |
        v
  Registry::handle_install_event()
        |
        v
  Registry::install_from_url()
    -> download -> read manifest -> move .napp -> extract binary/ui
    -> launch process -> register in tools map
```

### App Lifecycle (crates/server/src/app_lifecycle.rs)

```
  AppLifecycle::launch()
        |
        v
  1. Runtime::cleanup_stale()     -- kill orphan processes
  2. Runtime::launch()            -- spawn sidecar, wait for socket
  3. Supervisor::watch()          -- register for crash monitoring
  4. ClientHub::broadcast()       -- "app_started" WebSocket event
  5. discover_tools()             -- read agent.json, register sidecar tools
  6. SkillLoader::load_app_skills() -- load skills from tool_dir
  7. spawn_health_checker()       -- background task for liveness
```

### Plugin Tool (crates/tools/src/plugin_tool.rs)

```
  Agent calls plugin("gws", "exec", "gmail +triage")
        |
        v
  PluginTool::exec()
  1. PluginStore::resolve("gws", "*") -> binary path
  2. Build sanitized environment
  3. Spawn process with CLI args
  4. Read stdout, detect auth failures
  5. Auto-retry with re-auth if needed
  6. Return output to agent
```

### Agent Install Flow

```
  NeboAI -> download agent .napp
        |
        v
  1. Store at nebo/agents/<name>/<version>.napp
  2. extract_napp_alongside() -> nebo/agents/<name>/<version>/
  3. AgentLoader::load_all() picks up new directory
  4. AgentFsEvent::Added emitted via watcher
  5. Server syncs LoadedAgent into DB
  6. If is_app: AppLifecycle spawns sidecar
```

---

## 22. Error Handling

### NappError Enum

```rust
pub enum NappError {
    Manifest(String),              // Invalid manifest content
    Signing(String),               // Signature/key verification failure
    Extraction(String),            // Archive extraction failure
    Sandbox(String),               // Binary validation failure
    Runtime(String),               // Process launch/socket failure
    NotFound(String),              // Entry or file not found
    PermissionDenied(String),      // Access denied
    Revoked(String),               // Package revoked by NeboAI
    Io(std::io::Error),            // Filesystem I/O
    Http(reqwest::Error),          // Network request failure
    Json(serde_json::Error),       // JSON parse failure
    PluginNotFound(String),
    PluginPlatformUnavailable { plugin: String, platform: String },
    PluginDownloadFailed(String),
    PluginValidation(String),      // Manifest field validation
    Other(String),                 // Catch-all
}
```

### Error Categories

| Category         | Examples                                  | Recovery       |
|------------------|-------------------------------------------|----------------|
| Security         | Signing, Revoked, Extraction (tamper)     | Quarantine     |
| Corruption       | Extraction (hash mismatch)                | Re-download    |
| Network          | Http, Signing (fetch key)                 | Retry / cache  |
| Configuration    | Manifest, PluginValidation                | Fix content    |
| Runtime          | Runtime, Sandbox                          | Restart/fix    |
| Missing          | NotFound, PluginNotFound                  | Download       |

---

## 23. Security Model

### Threat Mitigations

| Threat                    | Mitigation                                          |
|---------------------------|-----------------------------------------------------|
| Tampered archive          | SHA256 hash in envelope header                      |
| Forged package            | ED25519 envelope signature                          |
| Modified binary           | SHA256 + ED25519 in signatures.json                 |
| Modified manifest         | ED25519 in signatures.json                          |
| Path traversal            | Reject "..", leading "/", canonicalize check         |
| Symlink attacks           | Reject all symlinks and hardlinks in tar             |
| Zip bombs / oversized     | Per-type size limits, double-checked                |
| Script injection          | Binary format validation (native only)              |
| Env var leakage           | env_clear() + allowlist + blocked list              |
| Stale credentials         | Revocation checker (1h cache, fail open)            |
| IP theft (sealed)         | AES-256-GCM, plaintext never on disk                |
| Runaway processes         | SIGTERM -> SIGKILL, process group isolation          |
| Crash loops               | Supervisor: 5/hour max, exponential backoff          |
| Hook failures             | Circuit breaker: 3 failures -> 5min disable          |
| Concurrent downloads      | Mutex-based deduplication                            |
| Socket sniffing           | Socket permissions 0o600                             |
| Supply chain (revocation) | Background revocation list polling                   |

### Trust Chain

```
  NeboAI signing key (ED25519 private, server-side)
        |
        | signs
        v
  .napp envelope (signature over hash || payload)
        |
        | contains
        v
  signatures.json
    manifest_signature  -- signs manifest.json
    binary_signature    -- signs binary
    binary_hash         -- SHA256 of binary
        |
        | verified by
        v
  Embedded public key (32 bytes, compile-time)
    OR
  Remote key (24h cache from /api/v1/apps/signing-key)
```

### Sealed Archive Protection

Sealed archives protect intellectual property (skill prose, agent personas):
- Only executables and metadata are extracted to disk
- Content (SKILL.md, AGENT.md, assets) read in-memory via sealed reader
- License key derived via HKDF-SHA256 from master secret + artifact_id
- HKDF info string: `b"neboai-license-v1"`
- Key derivation is deterministic and license-holder agnostic

---

## 24. Key Structs and Traits

### Core Types (re-exported from lib.rs)

```rust
pub use agent_loader::{AgentFsEvent, AgentLoader, AgentSource, LoadedAgent};
pub use hooks::{HookCaller, HookDispatcher, HookType, register_plugin_hooks};
pub use manifest::{Manifest, ManifestSignature, QualifiedName};
pub use registry::{Registry, RegistryConfig};
pub use runtime::{Process, Runtime};
pub use signing::{RevocationChecker, SigningKeyProvider, builtin_verifying_key};
```

### HookCaller Trait

```rust
#[async_trait]
pub trait HookCaller: Send + Sync {
    async fn call_filter(&self, hook: &str, payload: Vec<u8>) -> Result<(Vec<u8>, bool), String>;
    async fn call_action(&self, hook: &str, payload: Vec<u8>) -> Result<(), String>;
}
```

### Event Types

```rust
pub struct InstallEvent {
    pub event_type: String,    // tool_installed, tool_updated, tool_uninstalled, tool_revoked
    pub tool_id: String,
    pub payload: serde_json::Value,
}

pub struct QuarantineEvent {
    pub tool_id: String,
    pub reason: String,
}
```

---

## 25. File Locations on Disk

### Data Directory Layout

```
~/.nebo/data/
|
|-- nebo/                        Marketplace content (signed + sealed)
|   |-- agents/                  Installed agents
|   |   |-- @acme/agents/briefer/
|   |   |   |-- 1.0.0.napp      Sealed archive
|   |   |   +-- 1.0.0/          Extracted content
|   |   |       |-- manifest.json
|   |   |       |-- AGENT.md
|   |   |       |-- agent.json
|   |   |       +-- skills/
|   |   +-- ...
|   |
|   |-- plugins/                 Installed plugins
|   |   |-- gws/
|   |   |   |-- 1.2.0.napp      Sealed archive (optional)
|   |   |   +-- 1.2.0/          Extracted content
|   |   |       |-- gws          Binary
|   |   |       |-- plugin.json  Manifest
|   |   |       +-- skills/      Skill docs
|   |   |           |-- gmail/SKILL.md
|   |   |           +-- calendar/SKILL.md
|   |   +-- ...
|   |
|   +-- tools/                   Installed tools (gRPC sidecars)
|       |-- tool-id/
|       |   |-- 1.0.0.napp      Sealed archive
|       |   +-- 1.0.0/          Extracted binary + ui/
|       |       |-- manifest.json
|       |       |-- binary
|       |       |-- signatures.json
|       |       |-- data/        Runtime data
|       |       |   +-- sidecar.log
|       |       +-- tool-id.sock
|       +-- ...
|
|-- user/                        User-created content (no signatures)
|   |-- agents/                  User agents (loose files)
|   |   +-- my-agent/
|   |       |-- AGENT.md
|   |       +-- agent.json
|   |
|   |-- plugins/                 User plugins (override marketplace)
|   |   +-- my-plugin/
|   |       |-- plugin.json
|   |       +-- my-plugin
|   |
|   +-- tools/                   Sideloaded tools (symlinks to dev dirs)
|       +-- dev-tool -> /path/to/project/
|
+-- plugins/data/                Persistent plugin data (survives updates)
    +-- gws/
        +-- oauth-tokens.json
```

### Key Path Patterns

| Pattern                                      | Example                                                  |
|----------------------------------------------|----------------------------------------------------------|
| Sealed .napp archive                         | `nebo/agents/@acme/agents/brief/1.0.0.napp`             |
| Extracted directory                          | `nebo/agents/@acme/agents/brief/1.0.0/`                 |
| Plugin binary                                | `nebo/plugins/gws/1.2.0/gws`                            |
| Plugin data (persistent)                     | `plugins/data/gws/`                                      |
| Tool socket                                  | `nebo/tools/tool-id/1.0.0/tool-id.sock`                 |
| Tool PID file                                | `nebo/tools/tool-id/1.0.0/tool-id.pid`                  |
| Sidecar log                                  | `nebo/tools/tool-id/1.0.0/data/sidecar.log`             |
| Quarantine marker                            | `nebo/tools/tool-id/1.0.0/.quarantined`                  |
| User agent                                   | `user/agents/my-agent/AGENT.md`                          |
| Sideloaded tool (symlink)                    | `user/tools/dev-tool -> /home/dev/project/`              |
