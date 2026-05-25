# Secret Scanning & Redaction System -- SME Deep-Dive

> **Last updated:** 2026-05-15
>
> **Purpose:** Definitive technical reference for Nebo's two-layer secret protection system: (1) the pre-write secret scanner that prevents credentials from reaching persistent memory, and (2) the slash-command argument redaction layer that strips sensitive inputs before they enter chat history or logs. Covers all 15 regex detection patterns, every call site, cross-system interactions, and security design decisions.

---

## Key Files

| File | Purpose | Status |
|------|---------|--------|
| `crates/agent/src/secret_scan.rs` | Pre-write secret scanner: 15 compiled regex patterns, `contains_secret()`, `detect_secret()` | Active |
| `crates/server/src/redact.rs` | Slash-command argument redaction: 15 sensitive command prefixes, `redact_sensitive_args()` | Active |
| `crates/agent/src/memory.rs` | `store_facts()` calls `contains_secret()` to gate memory persistence | Active |
| `crates/agent/src/sanitize.rs` | Prompt injection detection and key/value sanitization (runs alongside secret scan) | Active |
| `crates/server/src/handlers/ws.rs` | WebSocket chat dispatch -- calls `redact_sensitive_args()` on inbound prompts | Active |
| `crates/server/src/handlers/agents.rs` | REST chat endpoint -- calls `redact_sensitive_args()` on inbound prompts | Active |
| `crates/agent/src/runner.rs` | Agent runner -- invokes `store_facts()` after memory extraction | Active |
| `crates/agent/src/memory_flush.rs` | Pre-compaction memory flush -- invokes `store_facts()` | Active |
| `crates/agent/src/memory_debounce.rs` | Debounced extraction scheduling (upstream of `store_facts()`) | Active |

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Two-Layer Protection Model](#2-two-layer-protection-model)
3. [Layer 1: Slash-Command Argument Redaction](#3-layer-1-slash-command-argument-redaction)
   - 3.1 [Sensitive Command List](#31-sensitive-command-list)
   - 3.2 [Redaction Logic](#32-redaction-logic)
   - 3.3 [Call Sites](#33-call-sites)
4. [Layer 2: Pre-Write Secret Scanner](#4-layer-2-pre-write-secret-scanner)
   - 4.1 [Detection Patterns](#41-detection-patterns)
   - 4.2 [Pattern Compilation Strategy](#42-pattern-compilation-strategy)
   - 4.3 [Public API](#43-public-api)
   - 4.4 [Call Sites](#44-call-sites)
5. [Pipeline Integration](#5-pipeline-integration)
   - 5.1 [Inbound Path (User Prompt)](#51-inbound-path-user-prompt)
   - 5.2 [Storage Path (Memory Persistence)](#52-storage-path-memory-persistence)
6. [Companion Systems](#6-companion-systems)
   - 6.1 [Sanitization Module](#61-sanitization-module)
   - 6.2 [Memory Debouncer](#62-memory-debouncer)
   - 6.3 [Memory Flush](#63-memory-flush)
7. [Security Design Decisions](#7-security-design-decisions)
8. [False Positive Analysis](#8-false-positive-analysis)
9. [Error Handling](#9-error-handling)
10. [Configuration and Extensibility](#10-configuration-and-extensibility)
11. [Known Limitations and Future Work](#11-known-limitations-and-future-work)

---

## 1. Architecture Overview

Nebo protects non-technical users from accidental credential leakage through two
independent defense layers that operate at different points in the data pipeline:

```
 User Input
     |
     v
+--------------------------------------------+
| LAYER 1: Slash-Command Redaction           |
| (crates/server/src/redact.rs)              |
|                                            |
|  "/auth sk-abc123" --> "/auth [redacted]"  |
|  "/help me"        --> unchanged           |
|                                            |
|  Intercept point: WebSocket handler +      |
|                   REST agent chat          |
|  Action: Replace arguments with [redacted] |
+--------------------------------------------+
     |
     v
  Chat Dispatch --> Agent Runner --> LLM Provider
     |                                    |
     |                            (response)
     |                                    |
     |                                    v
     |                          Memory Extraction
     |                           (extract_facts)
     |                                    |
     |                                    v
     |              +----------------------------------+
     |              | LAYER 2: Pre-Write Secret Scan   |
     |              | (crates/agent/src/secret_scan.rs) |
     |              |                                  |
     |              |  Each MemoryEntry.value checked  |
     |              |  against 15 credential regexes.  |
     |              |                                  |
     |              |  Action: SKIP storage entirely   |
     |              |  (entry never reaches SQLite)    |
     |              +----------------------------------+
     |                          |            |
     |                    [clean]        [secret]
     |                       |               |
     |                       v               v
     |               SQLite Storage       Dropped
     |              (upsert_memory)    (warn! logged)
     v
  Chat History
  (stored with redacted prompt)
```

**Key principle:** Defense in depth. Layer 1 catches secrets in structured slash
commands at the front door. Layer 2 catches secrets that the LLM might extract
from free-form conversation and attempt to persist as memory facts.

---

## 2. Two-Layer Protection Model

The two layers are completely independent -- neither depends on the other, and
both can operate in isolation. This design is intentional:

| Property | Layer 1 (Redaction) | Layer 2 (Secret Scan) |
|----------|--------------------|-----------------------|
| **Module** | `crates/server/src/redact.rs` | `crates/agent/src/secret_scan.rs` |
| **Scope** | User-submitted slash commands | LLM-extracted memory fact values |
| **Trigger** | Command prefix match (e.g. `/auth`) | Regex pattern match on content |
| **Action** | Replace args with `[redacted]` | Skip storage entirely (drop entry) |
| **Return type** | `Option<String>` (None = no change) | `bool` / `Option<&'static str>` |
| **Logging** | Implicit (caller handles) | `warn!` with key name |
| **Crate** | `nebo-server` | `nebo-agent` |
| **Detection method** | Exact command prefix match | 15 regex credential patterns |
| **False positive risk** | Very low (only known commands) | Moderate (generic patterns) |

---

## 3. Layer 1: Slash-Command Argument Redaction

### 3.1 Sensitive Command List

The system maintains a hardcoded list of 15 slash-command prefixes whose
arguments are considered sensitive. Matching is case-insensitive on the first
whitespace-delimited token:

```rust
// crates/server/src/redact.rs
const SENSITIVE_COMMANDS: &[&str] = &[
    "/auth",         // Authentication tokens
    "/login",        // Login credentials
    "/token",        // API tokens
    "/key",          // API keys
    "/secret",       // Client secrets
    "/password",     // Passwords
    "/apikey",       // API keys (no separator)
    "/api-key",      // API keys (hyphen)
    "/api_key",      // API keys (underscore)
    "/credential",   // Generic credentials
    "/credentials",  // Generic credentials (plural)
    "/oauth",        // OAuth tokens/secrets
    "/connect",      // Service connection strings
    "/register",     // Registration tokens/codes
    "/signup",       // Signup credentials
    "/signin",       // Sign-in credentials
];
```

### 3.2 Redaction Logic

```rust
pub fn redact_sensitive_args(prompt: &str) -> Option<String>
```

**Algorithm:**

1. Trim whitespace from input.
2. Early return `None` if the input does not start with `/`.
3. Extract the first whitespace-delimited token as the command.
4. Early return `None` if there are no arguments after the command.
5. Lowercase the command and match against `SENSITIVE_COMMANDS`.
6. If matched: return `Some("{command} [redacted]")`.
7. If not matched: return `None`.

The caller uses the `Option` return to decide whether to substitute:
```rust
let prompt = crate::redact::redact_sensitive_args(&prompt).unwrap_or(prompt);
```

This pattern preserves the original prompt when no redaction is needed, and
replaces it entirely when sensitive arguments are detected.

**Important ordering detail:** In the WebSocket handler, plugin dispatch happens
BEFORE redaction. This means the original, unredacted arguments are available to
the plugin system for authentication flows. Only after plugin dispatch completes
(or falls through) does redaction occur. The redacted version is what enters
chat storage and logs:

```
 User: "/auth sk-abc123"
     |
     +--> Plugin dispatch (receives "sk-abc123" -- original)
     |
     +--> Redaction (prompt becomes "/auth [redacted]")
     |
     +--> Chat storage (stores "/auth [redacted]")
```

### 3.3 Call Sites

**WebSocket handler** (`crates/server/src/handlers/ws.rs:1219`):
```rust
// Redact sensitive slash command arguments before the prompt enters storage
// or logs. The original args were already used for plugin dispatch above.
let prompt = crate::redact::redact_sensitive_args(&prompt).unwrap_or(prompt);
```

**REST agent chat** (`crates/server/src/handlers/agents.rs:2021`):
```rust
// Redact sensitive slash command arguments before storage
let prompt = crate::redact::redact_sensitive_args(&prompt).unwrap_or(prompt);
```

Both call sites follow the same pattern: unconditional call, `unwrap_or` fallback.

---

## 4. Layer 2: Pre-Write Secret Scanner

### 4.1 Detection Patterns

The scanner uses 15 compiled regex patterns targeting major cloud provider keys,
tokens, and private key material. Each pattern is a tuple of
`(&'static str, Regex)` where the string is a human-readable label used in
`detect_secret()` return values and log messages.

```
+----+------------------------+----------------------------------------------+
| #  | Label                  | Regex Pattern                                |
+----+------------------------+----------------------------------------------+
|  1 | AWS Access Key         | AKIA[0-9A-Z]{16}                             |
|  2 | AWS Secret Key         | (?i)aws_secret_access_key\s*=\s*\S{20,}     |
|  3 | OpenAI API Key         | sk-[A-Za-z0-9]{32,}                          |
|  4 | Anthropic API Key      | sk-ant-[A-Za-z0-9\-]{40,}                    |
|  5 | GitHub Token           | gh[pousr]_[A-Za-z0-9]{36,}                   |
|  6 | Generic API Key        | (?i)(api[_-]?key|apikey)\s*[:=]\s*            |
|    |                        |   ['"]?[A-Za-z0-9\-_.]{20,}                  |
|  7 | Bearer Token           | (?i)bearer\s+[A-Za-z0-9\-_.]{20,}            |
|  8 | Private Key            | -----BEGIN (RSA |EC |DSA |OPENSSH )?         |
|    |                        |   PRIVATE KEY-----                           |
|  9 | Slack Token            | xox[bprs]-[A-Za-z0-9\-]{10,}                 |
| 10 | Google API Key         | AIza[A-Za-z0-9\-_]{35}                       |
| 11 | Stripe Key             | (?:sk|pk)_(?:live|test)_[A-Za-z0-9]{20,}     |
| 12 | Twilio Auth Token      | (?i)twilio.*[0-9a-f]{32}                     |
| 13 | SendGrid Key           | SG\.[A-Za-z0-9\-_.]{22,}\.[A-Za-z0-9\-_.]   |
|    |                        |   {43}                                       |
| 14 | npm Token              | npm_[A-Za-z0-9]{36}                           |
| 15 | Heroku API Key         | (?i)heroku.*[0-9a-f]{8}-[0-9a-f]{4}-         |
|    |                        |   [0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}      |
+----+------------------------+----------------------------------------------+
```

**Pattern categories:**

- **Provider-specific prefixed keys** (patterns 1, 3, 4, 5, 9, 10, 11, 13, 14):
  These rely on known static prefixes (`AKIA`, `sk-`, `sk-ant-`, `gh[pousr]_`,
  `xox[bprs]-`, `AIza`, `sk_`/`pk_`, `SG.`, `npm_`). Very low false positive
  rate because the prefixes are distinctive.

- **Assignment patterns** (patterns 2, 6): Match `key = value` or `key: value`
  syntax. Moderate false positive risk due to generic matching.

- **Bearer token** (pattern 7): Matches `Bearer <token>` in authorization
  headers. Could match non-secret bearer references in documentation.

- **PEM markers** (pattern 8): Matches private key PEM headers. Very reliable --
  these are always sensitive.

- **Service-context patterns** (patterns 12, 15): Match UUIDs/hex strings in
  the context of service names (`twilio`, `heroku`). Higher false positive risk
  because they rely on proximity matching.

### 4.2 Pattern Compilation Strategy

Patterns are compiled once per process lifetime using `std::sync::OnceLock`:

```rust
fn patterns() -> &'static [(&'static str, Regex)] {
    static PATTERNS: OnceLock<Vec<(&str, Regex)>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let raw: [(&str, &str); 15] = [ /* ... */ ];
        raw.iter()
            .filter_map(|(name, pat)| {
                Regex::new(pat).ok().map(|r| (*name, r))
            })
            .collect()
    })
}
```

Design properties:
- **Thread-safe:** `OnceLock` guarantees single initialization across threads.
- **Zero runtime cost:** After first call, returns `&'static` reference.
- **Graceful degradation:** Invalid regex patterns are silently skipped via
  `filter_map` + `.ok()`. This means a malformed pattern reduces coverage
  rather than causing a panic.
- **No allocation after init:** The `Vec` lives for the process lifetime.

### 4.3 Public API

Two functions are exposed:

```rust
/// Returns true if the text contains any known secret patterns.
pub fn contains_secret(text: &str) -> bool

/// Returns the name of the first secret pattern found, if any.
pub fn detect_secret(text: &str) -> Option<&'static str>
```

`contains_secret` is the workhorse used in the memory storage gate. It
short-circuits on the first match via `.any()`.

`detect_secret` returns the human-readable label of the matched pattern. This
is useful for diagnostics but is not currently called from production code --
it exists for debugging and future use (e.g., reporting which pattern type
was detected to the user).

### 4.4 Call Sites

**Memory storage gate** (`crates/agent/src/memory.rs:231`):
```rust
// Skip entries that contain credential patterns (protect non-technical users)
if crate::secret_scan::contains_secret(&entry.value) {
    warn!(key = %entry.key, "skipping memory storage -- credential pattern detected");
    continue;
}
```

This check sits inside `store_facts()`, which is the sole path for persisting
extracted facts to SQLite. Every memory entry must pass this gate before reaching
`store.upsert_memory()`.

**Upstream callers of `store_facts()`:**

1. **Runner post-turn extraction** (`crates/agent/src/runner.rs:3470`):
   After each agentic turn completes, the runner debounces memory extraction.
   When extraction fires, `store_facts()` is called with the results.

2. **Pre-compaction memory flush** (`crates/agent/src/memory_flush.rs:201`):
   Before conversation compaction discards old messages, a flush runs extraction
   on all remaining messages and calls `store_facts()`.

Both paths funnel through the same `store_facts()` function, so the secret scan
gate covers 100% of automatic memory persistence.

---

## 5. Pipeline Integration

### 5.1 Inbound Path (User Prompt)

```
  User types: "/token sk-ant-api03-XXXXXXXXXX"
       |
       v
  [WebSocket handler: ws.rs]
       |
       +--> codes::detect_code()  -- not a marketplace code
       |
       +--> plugin_commands::try_dispatch()  -- plugin receives original args
       |    (If plugin handles it, response streamed. Otherwise fall through.)
       |
       +--> extract_images_from_prompt()
       |
       +--> redact::redact_sensitive_args()
       |    Input:  "/token sk-ant-api03-XXXXXXXXXX"
       |    Output: Some("/token [redacted]")
       |
       +--> prompt = "/token [redacted]"
       |
       +--> Empty check (not empty, proceed)
       |
       +--> chat_dispatch::run_chat() with redacted prompt
       |
       v
  [Chat history stores: "/token [redacted]"]
  [Agent sees: "/token [redacted]"]
```

### 5.2 Storage Path (Memory Persistence)

```
  Agent runner completes a turn
       |
       v
  [Memory debouncer: schedule extraction]
       |
       +--> Turn count >= 3?  AND  Tool calls >= 3?
       |    No:  skip extraction this turn
       |    Yes: schedule with 5-second idle delay
       |
       v (after 5s idle)
  [extract_facts() via LLM]
       |
       +--> LLM returns ExtractedFacts JSON
       |    {preferences: [...], entities: [...], ...}
       |
       v
  [store_facts()]
       |
       +--> format_for_storage() -- creates MemoryEntry vec
       |
       +--> For each MemoryEntry:
       |    |
       |    +--> secret_scan::contains_secret(&entry.value)?
       |    |    YES --> warn! log, continue (SKIP)
       |    |    NO  --> proceed
       |    |
       |    +--> sanitize::detect_prompt_injection(&entry.key)?
       |    |    YES --> debug! log, continue (SKIP)
       |    |    NO  --> proceed
       |    |
       |    +--> sanitize::detect_prompt_injection(&entry.value)?
       |    |    YES --> debug! log, continue (SKIP)
       |    |    NO  --> proceed
       |    |
       |    +--> entry.is_style?
       |    |    YES --> store_style_observation() (reinforcement)
       |    |    NO  --> store.upsert_memory() (direct write)
       |    |
       |    v
       |  [SQLite: memories table]
       |
       +--> embed_memories_async() for stored entries (if embedding provider)
```

### Three-Gate Memory Defense

Every memory entry must pass through three sequential checks before storage:

```
  MemoryEntry
      |
      v
  +-------------------+     +------------------------+     +-----------+
  | Secret Scan       | --> | Injection Detection    | --> | SQLite    |
  | (credential regex)|     | (prompt injection pat) |     | Storage   |
  | DROP if match     |     | DROP if match          |     |           |
  +-------------------+     +------------------------+     +-----------+
      |                          |
      v                          v
  [warn! logged]            [debug! logged]
  [entry dropped]           [entry dropped]
```

---

## 6. Companion Systems

### 6.1 Sanitization Module

`crates/agent/src/sanitize.rs` runs alongside the secret scanner in
`store_facts()`. While the secret scanner targets credential patterns, the
sanitizer handles two orthogonal concerns:

1. **Key/value sanitization** (`sanitize_memory_key`, `sanitize_memory_value`):
   Strips control characters and enforces length limits (128 chars for keys,
   2048 chars for values). Applied during `format_for_storage()` before the
   secret scan check.

2. **Prompt injection detection** (`detect_prompt_injection`): 14 regex patterns
   targeting jailbreak and instruction-override attempts. Checked on both key
   and value after the secret scan passes.

```
  format_for_storage()
      |
      +--> sanitize_memory_key()    -- control chars + truncation
      +--> sanitize_memory_value()  -- control chars + truncation
      |
      v
  store_facts() loop
      |
      +--> secret_scan::contains_secret()  -- credential check
      +--> sanitize::detect_prompt_injection() -- injection check (key)
      +--> sanitize::detect_prompt_injection() -- injection check (value)
```

### 6.2 Memory Debouncer

`crates/agent/src/memory_debounce.rs` controls when extraction fires, which
indirectly controls when the secret scanner runs. The debouncer enforces:

- **Turn interval:** Extraction only fires every 3 turns (`EXTRACTION_TURN_INTERVAL`).
- **Tool call threshold:** At least 3 tool calls must occur (`MIN_TOOL_CALLS`).
- **Idle delay:** 5-second delay after the last message before extraction runs.

This means the secret scanner is NOT called on every user message -- only when
the debouncer allows extraction to proceed. Short Q&A exchanges without tool
use will never trigger extraction (and therefore never trigger the secret scan).

### 6.3 Memory Flush

`crates/agent/src/memory_flush.rs` is the other path to `store_facts()`. It runs
before conversation compaction to salvage facts from messages about to be pruned.
The secret scan gate in `store_facts()` applies identically to flush-extracted
facts.

---

## 7. Security Design Decisions

### 7.1 Drop, Not Redact

The secret scanner **drops** entries entirely rather than redacting or masking
them. This is a deliberate choice:

- **No partial secrets:** A redacted value like `"API key: sk-***"` still leaks
  the key type and prefix. Dropping the entry reveals nothing.
- **No storage side effects:** The entry never reaches SQLite, the embedding
  pipeline, or the vector index. There is zero persistence of the secret.
- **Simplicity:** No need to decide what to mask or how much to reveal.

### 7.2 Value-Only Scanning

The secret scanner checks only `entry.value`, not `entry.key`. This is
intentional because:

- Keys are short identifiers (max 128 chars after sanitization) like
  `"preferred_editor"` or `"api_provider"`.
- Secrets appear in values where the LLM extracts the actual content.
- Scanning keys would increase false positives on legitimate key names like
  `"openai_preference"`.

### 7.3 OnceLock Compilation

Regex compilation happens once per process. The cost of compiling 15 patterns
is amortized over the lifetime of the server. This avoids both:

- Per-call compilation overhead (would be ~microseconds per pattern per call).
- Startup cost when the scanner might never be needed (lazy initialization).

### 7.4 Plugin Dispatch Before Redaction

In the WebSocket handler, slash commands are dispatched to plugins BEFORE
redaction occurs. This is critical for authentication flows:

```rust
// Plugin dispatch happens FIRST (needs original args)
if prompt.starts_with('/') {
    if let Some(result) = plugin_commands::try_dispatch(...).await {
        // ... handle plugin response
        return;
    }
}
// Redaction happens SECOND (only for storage/logging)
let prompt = crate::redact::redact_sensitive_args(&prompt).unwrap_or(prompt);
```

If redaction happened first, plugins like `/auth` would receive `[redacted]`
instead of the actual token, breaking authentication.

### 7.5 Two-Crate Separation

The redaction system lives in `nebo-server` while the secret scanner lives in
`nebo-agent`. This separation follows Nebo's dependency flow:

- `server` -> `agent`: Server can call agent code, but agent cannot call server code.
- Redaction operates at the HTTP/WebSocket boundary (server responsibility).
- Secret scanning operates at the memory persistence boundary (agent responsibility).

### 7.6 Defense Against LLM-Extracted Secrets

The most important threat model: a user pastes a `.env` file or config file into
chat. The LLM processes it, then the memory extractor might extract:

```json
{"key": "openai_api_key", "value": "sk-abc123def456..."}
```

Without the secret scanner, this would be persisted to SQLite and potentially
surfaced in future prompts. The scanner catches this at the `store_facts()` gate.

---

## 8. False Positive Analysis

### 8.1 Known False Positive Risks

| Pattern | False Positive Scenario | Risk Level |
|---------|------------------------|------------|
| OpenAI API Key (`sk-[A-Za-z0-9]{32,}`) | Any string starting with `sk-` followed by 32+ alphanumeric chars. Could match Stripe secret keys, random identifiers. | Medium |
| Generic API Key | Matches `api_key = <value>` in code discussions. May catch environment variable references that are not actual secrets. | Medium |
| Bearer Token | Matches `bearer <token>` in documentation discussions about HTTP auth. | Low-Medium |
| Twilio Auth Token | Matches `twilio` followed distantly by a 32-char hex string. Could match unrelated hex in Twilio discussions. | Medium |
| Heroku API Key | Matches `heroku` followed by any UUID-format string. Heroku resource IDs use UUIDs. | Medium |

### 8.2 Low-Risk Patterns

| Pattern | Why Low Risk |
|---------|-------------|
| AWS Access Key (`AKIA...`) | The `AKIA` prefix is unique to AWS access keys. |
| Anthropic API Key (`sk-ant-...`) | The `sk-ant-` prefix is distinctive. |
| GitHub Token (`gh[pousr]_...`) | GitHub token prefixes are unique. |
| Private Key PEM header | PEM headers are always sensitive content. |
| Stripe Key (`sk_live_`/`pk_live_`) | Stripe prefixes are distinctive. |
| SendGrid Key (`SG.xxx.xxx`) | Two-part dotted format is unique to SendGrid. |
| npm Token (`npm_...`) | The `npm_` prefix is distinctive. |
| Google API Key (`AIza...`) | The `AIza` prefix is unique to Google API keys. |
| Slack Token (`xox[bprs]-...`) | Slack token prefixes are distinctive. |

### 8.3 Impact of False Positives

When a false positive occurs, the memory entry is silently dropped. The user
will not be warned. The fact simply will not be remembered. For Nebo's target
audience (non-technical professionals), this is acceptable because:

- Memory loss for a single fact is not catastrophic (the fact can be re-stated).
- The alternative (storing a real secret) would be a security incident.
- The warn-level log (`warn!(key = %entry.key, "skipping memory storage -- credential pattern detected")`) allows developers to audit false positives.

---

## 9. Error Handling

### 9.1 Secret Scanner

The scanner is intentionally fail-safe:

- **Invalid regex patterns:** Silently skipped during compilation (`filter_map`
  with `.ok()`). Reduces coverage rather than panicking.
- **Empty input:** `patterns().iter().any(...)` returns `false` for empty
  strings. No special casing needed.
- **Unicode input:** Regex crate handles Unicode. Patterns use ASCII character
  classes, so Unicode text passes through without matching (correct behavior).

### 9.2 Redaction System

- **Non-slash input:** Returns `None` immediately (no allocation, no processing).
- **Command without arguments:** Returns `None` (e.g., `/auth` alone is not redacted because there is nothing to redact).
- **Mixed case:** Lowercases the command for comparison but preserves original
  case in output (`"/AUTH my-secret"` becomes `"/AUTH [redacted]"`).

### 9.3 Logging

| Event | Level | Location | Message |
|-------|-------|----------|---------|
| Secret detected in memory entry | `warn` | `memory.rs:232` | `"skipping memory storage -- credential pattern detected"` |
| Prompt injection in memory entry | `debug` | `memory.rs:240` | `"skipping memory entry due to injection detection: {key}"` |
| Memory extraction provider error | `warn` | `memory.rs:194` | `"memory extraction provider error: {e}"` |
| Failed to store memory entry | `debug` | `memory.rs:273` | `"failed to store memory entry {ns}/{key}: {e}"` |

The secret detection log is `warn` level (higher than the injection detection
`debug` level) because credential leakage is a more severe security concern.

---

## 10. Configuration and Extensibility

### 10.1 Current State: Hardcoded

Both systems use hardcoded pattern lists with no runtime configuration:

- **Secret scanner:** 15 patterns compiled into the binary via `OnceLock`.
- **Redaction:** 15 command prefixes as a `const` array.

There are no configuration files, environment variables, or database settings
to modify these lists at runtime.

### 10.2 Adding New Patterns

**Secret scanner:** Add a new tuple to the `raw` array in `patterns()`:
```rust
let raw: [(&str, &str); 16] = [  // Increment array size
    // ... existing patterns ...
    ("New Service Key", r#"new_prefix_[A-Za-z0-9]{24}"#),
];
```

**Redaction:** Add a new entry to `SENSITIVE_COMMANDS`:
```rust
const SENSITIVE_COMMANDS: &[&str] = &[
    // ... existing commands ...
    "/newservice",
];
```

Both require recompilation. There is no hot-reload mechanism.

### 10.3 No Sensitivity Levels

There is no concept of sensitivity levels, severity tiers, or configurable
thresholds. The system is binary: if a pattern matches, the entry is blocked
(secret scanner) or redacted (redaction). There is no "warn but allow" mode.

### 10.4 No Custom Patterns

Users cannot define custom secret patterns. This is a deliberate simplicity
choice for Nebo's non-technical target audience. Adding a user-facing pattern
editor would introduce complexity and potential misconfiguration risk.

---

## 11. Known Limitations and Future Work

### 11.1 No Scanning of Chat Messages

The secret scanner only operates on memory entries (extracted facts). If a user
pastes a secret into chat, it is stored in the chat history (messages table)
without scanning. Only the memory extraction path is gated.

The redaction layer mitigates this partially for slash commands, but free-form
secrets (e.g., "my API key is sk-abc123") in regular chat messages are stored
as-is in the messages table.

### 11.2 No Scanning of Tool Outputs

Tool call results (e.g., reading a `.env` file via the system tool) are stored
in chat history without secret scanning. If the LLM subsequently extracts a
credential from a tool output into a memory fact, the scanner will catch it at
the memory storage gate -- but the tool output itself remains in chat history.

### 11.3 Single-Pattern Matching in detect_secret

`detect_secret()` returns only the first matching pattern name. If text contains
multiple different secrets, only the first is reported. This is fine for the
current `contains_secret()` boolean gate but could be limiting for future
audit/reporting features.

### 11.4 No User Notification

When a memory entry is blocked by the secret scanner, the user receives no
notification. The entry is silently dropped. A future improvement could notify
the user: "A potential credential was detected and not saved to memory."

### 11.5 Overlap Between OpenAI and Stripe Patterns

The OpenAI pattern (`sk-[A-Za-z0-9]{32,}`) will also match Stripe secret keys
(`sk_live_...` would not match due to underscore, but `sk-live-...` would).
Both are legitimate secrets, so this is not a functional issue, but
`detect_secret()` would report the wrong label. The `contains_secret()` boolean
check is unaffected.

### 11.6 No Scanning of Memory Keys

As noted in Section 7.2, only `entry.value` is scanned. If the LLM places a
secret in the key field (unlikely but possible), it would not be caught. The
128-character key length limit from sanitization provides a partial mitigation
since most secrets exceed this when combined with descriptive key text, but
short secrets (e.g., short API keys) could theoretically fit.

---

## Appendix A: Test Coverage

### Secret Scanner Tests (`crates/agent/src/secret_scan.rs`)

| Test | What It Verifies |
|------|-----------------|
| `test_detects_aws_key` | AWS Access Key pattern (`AKIAIOSFODNN7EXAMPLE`) |
| `test_detects_openai_key` | OpenAI API Key pattern (`sk-abcdef...`) |
| `test_detects_anthropic_key` | Anthropic API Key pattern (`sk-ant-api03-...`) |
| `test_detects_private_key` | PEM private key header |
| `test_clean_text` | Normal text does NOT trigger (false negative check) |
| `test_clean_code` | Code with "api_key" in variable name does NOT trigger |

### Redaction Tests (`crates/server/src/redact.rs`)

| Test | What It Verifies |
|------|-----------------|
| `redacts_auth_command` | `/auth` with args is redacted |
| `redacts_case_insensitive` | `/AUTH`, `/Token` (mixed case) |
| `redacts_multiple_args` | All args replaced, not just first |
| `no_redaction_for_non_sensitive` | `/help`, `/gmail` pass through |
| `no_redaction_for_non_slash` | Regular text passes through |
| `no_redaction_without_args` | `/auth` alone (no args) passes through |
| `redacts_password_and_key` | `/password`, `/key`, `/secret` |
| `redacts_api_key_variants` | `/apikey`, `/api-key`, `/api_key` |

### Running Tests

```bash
# Secret scanner tests only
cargo test -p nebo-agent -- secret_scan

# Redaction tests only
cargo test -p nebo-server -- redact

# Both systems
cargo test -p nebo-agent -- secret_scan && cargo test -p nebo-server -- redact
```

---

## Appendix B: Full Function Signatures

### `crates/agent/src/secret_scan.rs`

```rust
/// Compiled regex patterns for common secrets.
/// Returns &'static slice, initialized once per process.
fn patterns() -> &'static [(&'static str, Regex)]

/// Returns true if the text contains any known secret patterns.
/// Short-circuits on first match.
pub fn contains_secret(text: &str) -> bool

/// Returns the name of the first secret pattern found, if any.
/// Returns None if no pattern matches.
pub fn detect_secret(text: &str) -> Option<&'static str>
```

### `crates/server/src/redact.rs`

```rust
/// Slash command prefixes whose arguments are considered sensitive.
const SENSITIVE_COMMANDS: &[&str]

/// If prompt is a sensitive slash command, return a copy with arguments
/// replaced by [redacted]. Otherwise return None.
pub fn redact_sensitive_args(prompt: &str) -> Option<String>
```

### `crates/agent/src/memory.rs` (relevant excerpt)

```rust
/// Store extracted facts in the database.
/// Style facts go through reinforcement, others are upserted directly.
/// Entries containing credential patterns are silently skipped.
pub fn store_facts(
    store: &Arc<Store>,
    facts: &ExtractedFacts,
    user_id: &str,
    embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
)
```
