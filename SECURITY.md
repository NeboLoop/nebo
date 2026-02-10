# Security

This document describes the security architecture and safety measures in Nebo.

---

## Overview

Nebo runs an AI agent with access to your computer's file system, shell, and network. Security is enforced at **multiple independent layers** so that no single bypass can compromise your system. The design principle is **defense in depth** — every layer assumes the one above it might fail.

```
┌──────────────────────────────────────────────────────┐
│  Hard Safeguard (unconditional, no bypass possible)  │  ← System paths, sudo, disk formatting
├──────────────────────────────────────────────────────┤
│  Origin-Based Restrictions (per request source)      │  ← Apps/skills can't use shell
├──────────────────────────────────────────────────────┤
│  Tool Policy & Approval (user-configurable)          │  ← Allowlist, approval prompts
├──────────────────────────────────────────────────────┤
│  Capability Permissions (per-category toggles)       │  ← File, shell, web, etc.
├──────────────────────────────────────────────────────┤
│  App Sandbox (for third-party apps)                  │  ← Env isolation, signing, permissions
├──────────────────────────────────────────────────────┤
│  Network Security (auth, CSRF, headers, rate limits) │  ← JWT, CORS, CSP, rate limiting
└──────────────────────────────────────────────────────┘
```

---

## 1. Hard Safeguard (Cannot Be Overridden)

**File:** `internal/agent/tools/safeguard.go`

The safeguard is an unconditional safety rail that runs inside `registry.Execute()` before every tool call. It **cannot be bypassed** by autonomous mode, policy level, user approval, or any setting.

### Blocked Operations

#### Privilege Escalation
- **sudo** — Blocked in all forms: direct, piped, chained, subshell. Nebo must never run commands with elevated privileges.
- **su** — Blocked entirely. Nebo must never switch to another user.

#### Disk Formatting & Partitioning
- **mkfs** (all variants) — Cannot format filesystems
- **fdisk / gdisk / cfdisk / sfdisk / sgdisk** — Cannot modify partition tables
- **parted** — Cannot modify disk partitions
- **wipefs** — Cannot wipe filesystem signatures
- **diskutil eraseDisk / eraseVolume / partitionDisk** (macOS) — Cannot erase or partition disks
- **dd to block devices** (`of=/dev/...`) — Cannot write raw data to drives

#### Filesystem Destruction
- **rm -rf /** (all variants including `--no-preserve-root`) — Cannot delete root filesystem
- **Writing to /dev/** — Blocked except `/dev/null`, `/dev/stdout`, `/dev/stderr`
- **Fork bombs** — Detected and blocked

#### Protected System Paths (No Writes or Edits)

All read operations are allowed. Only write/edit operations are blocked.

| macOS | Linux | Windows |
|-------|-------|---------|
| `/System/` | `/bin/`, `/sbin/` | `C:\Windows\` |
| `/bin/`, `/sbin/` | `/usr/bin/`, `/usr/sbin/`, `/usr/lib/` | `C:\Program Files\` |
| `/usr/bin/`, `/usr/sbin/`, `/usr/lib/` | `/boot/` | `C:\Program Files (x86)\` |
| `/etc/` | `/etc/` | `C:\ProgramData\` |
| `/Library/LaunchDaemons/` | `/proc/`, `/sys/`, `/dev/` | `C:\Recovery\` |
| `/Library/LaunchAgents/` | `/root/` | |
| `/private/var/db/` | `/var/lib/dpkg/`, `/var/lib/rpm/` | |

#### Nebo's Own Data Directory (No Writes or Deletes)
Nebo cannot modify or delete its own database, WAL, or data files. This prevents catastrophic self-harm where the agent destroys its own persistence.

| Platform | Protected Path |
|----------|---------------|
| macOS | `~/Library/Application Support/Nebo/data/` |
| Linux | `~/.config/nebo/data/` |
| Windows | `%APPDATA%\Nebo\data\` |
| Override | `$NEBO_DATA_DIR/data/` |

This includes `nebo.db`, `nebo.db-wal`, `nebo.db-shm`, and any other files in the data directory.

#### Sensitive User Paths (No Writes)
- `~/.ssh/` — SSH keys and configuration
- `~/.gnupg/` — GPG keys and configuration
- `~/.aws/credentials` — AWS credentials
- `~/.kube/config` — Kubernetes credentials
- `~/.docker/config.json` — Docker registry credentials

#### rm / chmod / chown on System Paths
When `rm`, `chmod`, or `chown` commands target any protected system directory, they are blocked unconditionally.

### What To Do If You Need These Operations
The safeguard error messages tell the user to perform the operation manually in a terminal. Nebo is not designed for system administration — it helps with development and productivity tasks within user-space directories.

---

## 2. Origin-Based Tool Restrictions

**File:** `internal/agent/tools/origin.go`

Every request is tagged with an origin that tracks where it came from. Non-user origins have hard restrictions:

| Origin | Source | Restrictions |
|--------|--------|-------------|
| `user` | Direct interaction (web UI, CLI) | None (governed by policy) |
| `system` | Internal tasks (heartbeat, cron, recovery) | None |
| `comm` | Inter-agent communication | **Shell denied** |
| `app` | External app binary | **Shell denied** |
| `skill` | Matched skill template | **Shell denied** |

These are **hard denies** — no approval prompt, no override. A remote agent or app cannot execute shell commands on your machine.

---

## 3. Tool Policy & Approval System

**File:** `internal/agent/tools/policy.go`

### Policy Levels
- **Allowlist** (default) — Only whitelisted commands auto-approve; everything else prompts
- **Deny** — All operations require approval
- **Full** — All operations auto-approve (dangerous, opt-in only)

### Approval Modes
- **On-miss** (default) — Prompt when command is not in the allowlist
- **Always** — Prompt for every operation
- **Off** — Never prompt

### Safe Commands (Auto-Approved)
Read-only and inspection commands: `ls`, `pwd`, `cat`, `head`, `tail`, `grep`, `find`, `which`, `jq`, `cut`, `sort`, `uniq`, `wc`, `echo`, `date`, `env`, `git status`, `git log`, `git diff`, `git branch`, `git show`, `go version`, `node --version`, `python --version`

### Dangerous Command Detection
The `IsDangerous()` function flags: `rm -rf`, `rm -r`, `sudo`, `su`, `chmod 777`, `chown`, `dd`, `mkfs`, `curl | sh`, `wget | sh`, `eval`, `exec`, fork bombs.

### Autonomous Mode
When enabled, bypasses all approval prompts. Requires:
1. Accepting terms of service with explicit warnings
2. Typing "ENABLE" to confirm
3. Acknowledging liability

When disabled, **all permissions reset to defaults** (only Chat & Memory enabled).

### Approval Queue
Multiple concurrent tool approval requests (from different lanes) queue up instead of overwriting each other. The user sees them one at a time, in order.

---

## 4. Capability Permissions

Users can enable/disable entire capability categories:

| Category | What It Controls |
|----------|-----------------|
| Chat & Memory | Conversations, memory, scheduled tasks (always on) |
| File System | Read, write, edit, search files |
| Shell & Terminal | Execute commands, manage processes |
| Web Browsing | Fetch pages, search, browser automation |
| Contacts & Calendar | Contacts, calendar, reminders, mail |
| Desktop Control | Window management, accessibility, clipboard |
| Media & Capture | Screenshots, image analysis, music, TTS |
| System | Spotlight, keychain, Siri shortcuts, notifications |

Disabled categories prevent the tool from being registered with the agent entirely — the LLM never sees it as an available tool.

---

## 5. App Sandbox Security

Third-party apps (`.napp` packages) run in a sandboxed environment.

### Environment Isolation (`internal/apps/sandbox.go`)
- **Allowlist-only environment**: Only `PATH`, `HOME`, `TMPDIR`, `LANG`, `LC_ALL`, `TZ`, and `NEBO_APP_*` variables are passed to apps
- All parent environment variables (API keys, secrets, tokens) are **stripped**
- Per-app isolated log files with `0600` permissions

### Binary Validation
- Rejects symlinks (path traversal prevention)
- Rejects non-regular files (devices, pipes)
- Checks executable bit
- Enforces size limit (500MB default)

### App Signing (`internal/apps/signing.go`)
- **ED25519** signature verification over raw bytes
- Manifest signature: verifies manifest.json integrity
- Binary signature: SHA256 integrity check + ED25519 verification
- Key ID rotation detection (`SHA256(publicKey)[:8]`)
- Signing key cache (24-hour TTL, force-refresh on failure)
- Revocation checking (1-hour cache TTL)

### Secure .napp Extraction (`internal/apps/napp.go`)
Seven-layer defense:
1. Symlink rejection
2. Hard link rejection
3. Path traversal detection (`..` and absolute paths blocked)
4. Path escape check (target must stay within destination directory)
5. Filename allowlist (only `manifest.json`, `binary`, `signatures.json`, `ui/*`)
6. Size limits (binary: 500MB, UI files: 5MB, metadata: 1MB)
7. Required file validation (all three core files must be present)

### Deny-by-Default Permissions (`internal/apps/manifest.go`)
Apps must declare every permission they need. Unknown permissions are rejected. Categories include:
- `network:*` (DNS, HTTP, WebSocket)
- `filesystem:read`, `filesystem:write`
- `shell:execute`, `shell:background`
- `memory:read`, `memory:write`
- `session:*`, `tool:*`, `model:*`, `channel:*`, `mcp:*`

Permission changes on app updates require user re-approval.

---

## 6. Network Security

### Authentication
- **JWT** (HMAC-SHA256) for all API requests
- **WebSocket authentication required** — unauthenticated connections rejected with 401
- **Password hashing**: bcrypt with cost factor 12
- **Password requirements**: 8+ characters, uppercase, lowercase, digit, special character

### CSRF Protection (`internal/middleware/csrf.go`)
- Random 32-byte tokens with HMAC-SHA256 signing
- **Constant-time comparison** (prevents timing attacks)
- 12-hour token expiry
- `SameSite: Strict` cookies
- Double-submit cookie option available

### Security Headers (`internal/middleware/security_headers.go`)
- **Content-Security-Policy**: `default-src 'self'; script-src 'self'; object-src 'none'; frame-ancestors 'none'`
- **X-Frame-Options**: `DENY` (clickjacking prevention)
- **X-Content-Type-Options**: `nosniff` (MIME sniffing prevention)
- **Strict-Transport-Security**: 1-year HSTS with preload
- **Referrer-Policy**: `strict-origin-when-cross-origin`
- **Permissions-Policy**: Blocks camera, microphone, geolocation, payment, USB
- **Cache-Control**: `no-store, no-cache, must-revalidate, private`

### CORS (`internal/middleware/cors.go`)
- Whitelist-based origin validation (not `*`)
- Wildcard subdomain support
- Preflight method and header validation

### Rate Limiting (`internal/middleware/ratelimit.go`)
- Token bucket algorithm, per-client
- General: 100 requests/minute, burst 20
- Auth endpoints: 5 requests/minute, burst 5
- Trusted proxy support (only trusts `X-Forwarded-For` from known proxy IPs)
- Automatic cleanup of stale entries

### Request Validation (`internal/middleware/validation.go`)
- Maximum URL length: 2048 characters
- Maximum body size: 10MB
- Request size enforcement via `http.MaxBytesReader()`

### Input Sanitization (`internal/security/`)
- **SQL injection detection**: 40+ regex patterns covering comments, keyword injection, boolean injection, time-based injection, hex encoding, null bytes, stacked queries
- **XSS detection**: HTML tag and script pattern matching
- **Identifier validation**: Alphanumeric + underscore only, max 128 chars, reserved word blocking
- **Sanitization**: HTML entity escaping, control character removal, null byte stripping, UTF-8 validation

### SQL Injection Prevention
All database queries use **sqlc-generated parameterized queries** (prepared statements). No raw string concatenation in SQL.

### Encryption
- **MCP tokens**: AES-256-GCM with random nonces (`internal/mcp/client/crypto.go`)
- Key derivation from `MCP_ENCRYPTION_KEY`, `JWT_SECRET`, or persistent key file (`0600` permissions)

---

## 7. Process Safety

### Single Instance Lock
Only one Nebo instance per computer. Uses filesystem locks (`flock` on Unix, `LockFileEx` on Windows) with PID tracking.

### Process Isolation for Apps
- Each app runs as a separate process with its own Unix domain socket
- Sanitized environment (no inherited secrets)
- Per-app log files with restricted permissions
- Health checks with timeouts

### WebSocket Limits
- Maximum message size: 32KB
- Read deadline: 60 seconds (with ping/pong keepalive)
- Buffered send channel (256 messages)
- Graceful close on connection drop

---

## 8. Vulnerability Hardening Log

This section tracks every security vulnerability identified during development, how each was resolved, and which remain open. Vulnerabilities are sourced from internal audits, the OpenClaw comparison analysis, and the MAESTRO threat framework.

### Critical Severity — Resolved

#### CSWSH: Cross-Site WebSocket Hijacking
**Status:** CLOSED | **Files:** `internal/middleware/cors.go`, `internal/agenthub/hub.go`, `internal/websocket/handler.go`

Both WebSocket endpoints (`/ws`, `/api/v1/agent/ws`) had `CheckOrigin: return true`, allowing any webpage to establish connections and control the agent remotely.

**Hardening:** Added `IsLocalOrigin()` helper to `middleware/cors.go` that validates the `Origin` header against `localhost`/`127.0.0.1`. Empty `Origin` is allowed for direct CLI/agent connections. Applied to both the agent WebSocket (`agenthub/hub.go`) and chat WebSocket (`websocket/handler.go`). The gateway proxy was left unchanged because it uses separate token-based device authentication. Browser relay CDP upgraders already had localhost validation.

---

#### Shell Injection: No Sandbox or Env Sanitization
**Status:** CLOSED | **Files:** `internal/agent/tools/shell_tool.go`, `internal/agent/tools/shell_unix.go`, `internal/agent/tools/process_registry.go`

`bash -c` ran directly on the host with no sandbox. No environment variable sanitization — `PATH`, `LD_PRELOAD`, `DYLD_INSERT_LIBRARIES` all inherited from the parent process. Policy allowlist used prefix matching bypassable with semicolons.

**Hardening:** Added `sanitizedEnv()` function that strips 30+ dangerous environment variables before every shell execution:
- **Linux linker injection:** `LD_PRELOAD`, `LD_LIBRARY_PATH`, `LD_AUDIT`, and all `LD_*` prefixed vars
- **macOS linker injection:** `DYLD_INSERT_LIBRARIES`, `DYLD_LIBRARY_PATH`, `DYLD_FRAMEWORK_PATH`, and all `DYLD_*` prefixed vars
- **Shell behavior manipulation:** `IFS`, `CDPATH`, `BASH_ENV`, `PROMPT_COMMAND`, `SHELLOPTS`, `BASHOPTS`, `GLOBIGNORE`
- **ShellShock-style function exports:** `BASH_FUNC_*` prefix
- **Interpreter injection:** `PYTHONSTARTUP`, `NODE_OPTIONS`, `RUBYOPT`, `PERL5OPT`, `PERL5DB`
- **DNS manipulation:** `HOSTALIASES`, `RESOLV_HOST_CONF`

Applied to both foreground (`handleBash`) and background (`SpawnBackgroundProcess`) execution paths. `ShellCommand()` now uses absolute `/bin/bash` path with fallback to `/usr/bin/bash` and `/usr/local/bin/bash`, preventing `PATH`-based binary substitution attacks. 5 new tests pass.

---

#### SSRF: Server-Side Request Forgery in Web Fetch
**Status:** CLOSED | **Files:** `internal/agent/tools/web_tool.go`

`web_tool.go handleFetch` accepted any URL with no validation — no checks for private IPs, localhost, cloud metadata endpoints, or non-HTTP schemes. Default `http.Client` followed redirects to internal IPs.

**Hardening:** Added comprehensive SSRF protection with 3 layers:
1. **Pre-flight validation** (`validateFetchURL()`): Validates URL scheme (http/https only), resolves hostname via DNS, blocks all private/internal IP ranges (`127.0.0.0/8`, `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `169.254.0.0/16`, `0.0.0.0/8`, `100.64.0.0/10`, `::1/128`, `fc00::/7`, `fe80::/10`)
2. **Connection-time validation** (`ssrfSafeTransport()`): Custom dialer re-validates resolved IPs at connection time, catching DNS rebinding attacks and redirect-based SSRF
3. **Redirect validation** (`ssrfSafeRedirectCheck()`): Validates every redirect target against the same rules, with a 10-redirect cap

25 test cases covering public URLs, all blocked IP ranges, scheme validation, and edge cases.

---

#### DNS Hijack: local.nebo.bot Domain References
**Status:** CLOSED | **Files:** 15 files across codebase

All references to `local.nebo.bot` posed a DNS hijack risk — if anyone gained control of the `nebo.bot` domain, they could serve malicious pages passing CORS/origin checks on every Nebo instance.

**Hardening:** Replaced all references to `local.nebo.bot` with `localhost` across 15 files. Default domain is now `localhost`, default BaseURL is `http://localhost:27895`. Removed from: config defaults, vite dev server, server config, env example, browser relay origin check, chrome extension manifest and storage, agent config, provider CLI output, profile handler, default config.yaml, documentation.

---

#### No Origin Tagging on Sessions/Messages
**Status:** CLOSED | **Files:** `internal/agent/tools/origin.go`, `internal/agenthub/lane.go`, `internal/agent/runner/runner.go`

Without origin tagging, there was no way to distinguish a user typing a command from a remote MQTT message or app output triggering the same command.

**Hardening:** Implemented origin tagging system with 5 origin types (`user`, `comm`, `app`, `skill`, `system`). Every request carries an origin through context propagation (`WithOrigin(ctx, origin)` / `GetOrigin(ctx)`). The origin is set at the entry point (WebSocket handler, MQTT listener, app adapter, skill matcher, cron scheduler) and flows through the entire execution chain.

---

#### No Origin-Aware Tool Policy
**Status:** CLOSED | **Files:** `internal/agent/tools/policy.go`, `internal/agent/tools/registry.go`

`registry.Execute()` only checked policy level (full/approve/readonly) but not who was requesting. Remote agents, apps, and skills could access any tool.

**Hardening:** Extended the tool registry to check session origin before allowing tool execution. `Policy.IsDeniedForOrigin()` is checked before any approval logic. Configurable deny lists per origin with secure defaults.

---

#### Default-Deny for Dangerous Tools on Non-User Origins
**Status:** CLOSED | **Files:** `internal/agent/tools/policy.go`

Without default restrictions, comm/app/skill origins had full access to shell, file writes, memory, cron, and sub-agent spawning.

**Hardening:** Set secure defaults that block high-risk tools for non-user origins:
- **comm/channel origins:** Shell (all actions) denied — neutralizes NeboLoop as a remote shell vector
- **app origins:** Shell (all actions) denied — app outputs re-entering the loop cannot escalate
- **skill origins:** Shell (all actions) denied — malicious skill templates cannot trigger dangerous tools

Users can override via `config.yaml` for specific trusted integrations. This single change neutralizes three attack vectors: (1) NeboLoop as remote shell, (2) app injection escalation, (3) skill supply chain attacks.

---

### Critical Severity — Open

#### Credential Storage — Partial Encryption
**Priority:** Critical | **Files:** `internal/db/migrations/0010_auth_profiles.sql`, `internal/handler/provider/`, `internal/mcp/client/crypto.go`

`models.yaml` does NOT store API keys (uses `${ENV_VAR}` placeholders only). However, the `auth_profiles` table stores LLM provider API keys (Anthropic, OpenAI, Google, etc.) and NeboLoop tokens in **plaintext**. The `channel_credentials` table also stores bot tokens in plaintext.

MCP OAuth tokens ARE encrypted (AES-256-GCM via `crypto.go`), but the encryption key sits on disk in `~/.config/nebo/.mcp-key` — if an attacker can read the database, they can read the key file too.

**The real fix is OS Keychain integration** (macOS Keychain, Windows DPAPI, Linux libsecret). File-based encryption keys provide no meaningful protection against a local attacker with filesystem access. This is the only approach that ties credential access to the user's login session.

#### Secure Chrome Extension Relay CDP Endpoint
**Priority:** Critical | **Files:** `internal/browser/relay.go`

CDP endpoint allows unauthenticated loopback connections. Any local process can take full browser control, steal cookies, extract session tokens. Needs auth token requirement for all connections, connection rate limiting, and CDP command logging.

#### Sanitize Web Content Before LLM Processing
**Priority:** Critical | **Files:** `internal/agent/tools/web_tool.go`, `internal/browser/snapshot.go`

`web_tool.go` returns raw HTML to the LLM with only length truncation, no sanitization. No HTML stripping, no script removal. Malicious webpages can inject instructions into agent context. Needs HTML tag stripping, script/style removal, and special character escaping before returning content to the LLM.

---

### High Severity — Resolved

#### Path Traversal in File Tools
**Status:** CLOSED | **Files:** `internal/agent/tools/file_tool.go`

`file_tool.go` read/write/edit had no path restrictions. The agent could read `~/.ssh/id_rsa`, `~/.aws/credentials`, `/etc/passwd`. Write could target `~/.bashrc`, `~/.ssh/authorized_keys`. No symlink protection. Dangerous-path checks only existed for grep, not read/write/edit.

**Hardening:** Added `validateFilePath()` with a `sensitivePaths` blocklist (~25 paths including `~/.ssh`, `~/.aws`, `~/.config/gcloud`, `~/.azure`, `~/.gnupg`, `~/.docker/config.json`, `~/.kube/config`, `~/.npmrc`, `~/.password-store`, `~/Library/Keychains`, browser profiles, shell rc files, `/etc/shadow`, `/etc/passwd`, `/etc/sudoers`). Symlink resolution via `filepath.EvalSymlinks` prevents symlink-based traversal. `pathMatchesOrIsInside()` helper checks both exact match and directory containment. Applied to `handleRead`, `handleWrite`, and `handleEdit`. 12 tests pass.

---

#### Dev-Login Endpoint Bypass
**Status:** CLOSED | **Files:** `internal/handler/auth/devloginhandler.go` (deleted)

Nebo-specific issue. `/api/v1/auth/dev-login` returned valid JWT for hardcoded test emails without password verification.

**Hardening:** Deleted entirely rather than gating behind an environment variable. The existing auth system (register, login, setup wizard) covers all use cases. Removed: handler, route from `server.go`, frontend page, API client function. Ran `make gen` to confirm clean regeneration.

---

#### App/Skill Signing Trust Chain
**Status:** PARTIALLY CLOSED | **Files:** `internal/apps/signing.go`, `internal/apps/runtime.go`, `internal/apps/registry.go`, `internal/apps/install.go`

**What's done (App signing):**
- ED25519 signature verification over raw bytes for both manifest and binary
- `SigningKeyProvider` with 24-hour cache TTL and force-refresh on failure
- `RevocationChecker` with 1-hour cache TTL
- Runtime enforcement: Launch() checks revocation → verifies signatures → validates binary before executing
- Quarantine mechanism: stops process, removes binaries, preserves data/logs, writes `.quarantined` marker
- Periodic revocation sweep: background goroutine checks all running apps every hour
- MQTT `app_revoked` event handler for immediate kill switch
- Comprehensive tests with real ED25519 keys

**What's still needed:** Skills signature verification in `internal/agent/skills/loader.go`, wiring `NeboLoopURL` to `AppRegistry` in `cmd/nebo/agent.go`.

---

### High Severity — Open

#### Memory Prompt Injection
**Priority:** High | **Files:** `internal/agent/memory/dbcontext.go`, `internal/agent/memory/extraction.go`

`memory/dbcontext.go` concatenates stored memories directly into the system prompt with zero escaping. Time-shifted prompt injection is viable — an attacker stores a malicious payload in memory, it activates later. Needs sanitization of all stored values, structured delimiters (XML/JSON tags), and content-type validation for extracted memories.

#### Compaction Summary Poisoning (ARCH-002)
**Priority:** High | **Files:** `internal/agent/runner/compaction.go`, `internal/agent/session/`

Compaction summaries are prepended to the system prompt for future turns. A poisoned conversation (via prompt injection from web content, comm messages, or app output) gets compressed into a summary that persists across sessions. Needs structured state snapshots instead of free-form prose, a summary sanitizer, provenance tracking, and fallback behavior for suspicious patterns.

---

### Medium Severity — Resolved

#### X-Forwarded-For Header Spoofing in Rate Limiter
**Status:** CLOSED | **Files:** `internal/middleware/ratelimit.go`, `internal/middleware/ratelimit_test.go`

`middleware/ratelimit.go` blindly trusted `X-Forwarded-For` for rate limit keying. Attackers could bypass auth rate limits (5 attempts/min) by rotating the header value.

**Hardening:** `DefaultKeyFunc` now uses only `r.RemoteAddr`, ignoring `X-Forwarded-For` and `X-Real-IP` entirely since any client can spoof these headers. Added `TrustedProxyKeyFunc(trustedProxies []string)` for deployments behind a known reverse proxy — it only trusts forwarded headers when `RemoteAddr` matches a whitelisted proxy IP. Added `splitHostPort` helper for IPv4/IPv6 address parsing. 10 rate limiter tests pass.

---

#### CORS Wildcard Access-Control-Allow-Origin
**Status:** CLOSED | **Files:** `internal/middleware/cors.go`, `internal/middleware/security.go`

Security middleware previously used an empty `AllowedOrigins` list, providing no explicit allowlist.

**Hardening:** `DefaultCORSConfig` now returns localhost-only origins (`localhost:27895`, `localhost:5173`). Removed `local.nebo.bot` to prevent DNS hijack attacks. Added `BaseURL`-derived origin fallback in `security.go` for production deployments. Three tiers: explicit `AllowedOrigins` config > `BaseURL`-derived > localhost defaults.

---

### Medium Severity — Open

#### Localhost Endpoint Authentication Audit
**Priority:** Medium | **Files:** `internal/server/server.go`, `internal/middleware/chi_jwt.go`

Any local process can hit the Nebo HTTP API on port 27895. While JWT auth exists on protected routes, a full audit is needed to verify: all endpoints require auth tokens, no endpoints rely solely on origin/IP checks, WebSocket upgrade endpoints require valid JWT, and static file serving doesn't leak sensitive data.

#### Sub-Agent Rate Limiting (ARCH-008)
**Priority:** Medium | **Files:** `internal/agent/orchestrator/orchestrator.go`, `internal/agenthub/lane.go`

The subagent lane has no concurrency limit (set to 0 = unlimited). A comm-origin task or prompt injection could spawn many sub-agents to brute-force past restrictions or consume resources. Needs max concurrent sub-agent caps, per-origin rate limits, wall clock time caps for comm-origin sessions, and total spawn count limits.

#### Media/File Upload Sanitization (MAESTRO LM-004)
**Priority:** Low | **Files:** To be determined

Prompt injection via file uploads and media — hidden text in image EXIF metadata, invisible PDF text layers, zero-font-size text, CSS-hidden injection payloads. Needs metadata stripping, visible-text-only extraction, and source markers for media-derived content.

#### Audit Log Integrity Protection (MAESTRO EO-004)
**Priority:** Low | **Files:** `internal/db/`

SQLite stores logs with no integrity protection. An attacker with filesystem write access could modify or delete log entries. Planned approach: append-only triggers, hash-chained log entries (SHA-256), and eventual remote log shipping to NeboLoop.

---

## Reporting Security Issues

If you discover a security vulnerability, please report it responsibly:

1. **Do not** open a public GitHub issue
2. Email security concerns to the maintainers directly
3. Include steps to reproduce the vulnerability
4. Allow reasonable time for a fix before disclosure

---

## Design Principles

1. **Defense in depth** — Multiple independent layers, each assuming others might fail
2. **Fail closed** — When in doubt, block the operation
3. **Least privilege** — Deny by default, require explicit opt-in
4. **No silent failures** — Blocked operations explain why and what the user can do instead
5. **Unconditional safety** — Critical protections (system paths, sudo, disk formatting) cannot be overridden by any setting, mode, or configuration
