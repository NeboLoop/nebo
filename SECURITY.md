# Security

This document describes the security architecture and safety measures in Nebo.

---

## Overview

Nebo runs an AI agent with access to your computer's file system, shell, and network. Security is enforced at **multiple independent layers** so that no single bypass can compromise your system. The design principle is **defense in depth** — every layer assumes the one above it might fail.

```
+------------------------------------------------------+
|  Hard Safeguard (unconditional, no bypass possible)   |  <- System paths, sudo, disk formatting
+------------------------------------------------------+
|  Origin-Based Restrictions (per request source)       |  <- Infrastructure ready, deny list pending
+------------------------------------------------------+
|  Tool Policy & Approval (user-configurable)           |  <- Allowlist, approval prompts
+------------------------------------------------------+
|  Capability Permissions (per-category toggles)        |  <- File, shell, web, etc.
+------------------------------------------------------+
|  App Sandbox (for third-party apps)                   |  <- Env isolation, signing, permissions
+------------------------------------------------------+
|  Compiled-Only Binary Policy (anti-self-modification) |  <- No interpreted languages, opaque binaries
+------------------------------------------------------+
|  Network Security (auth, CSRF, headers, rate limits)  |  <- JWT, CORS, CSP, rate limiting
+------------------------------------------------------+
```

---

## 1. Hard Safeguard (Cannot Be Overridden)

**File:** `crates/tools/src/safeguard.rs`

The safeguard is an unconditional safety rail that runs inside `Registry::execute()` before every tool call. It **cannot be bypassed** by autonomous mode, policy level, user approval, or any setting.

### Blocked Operations

#### Privilege Escalation
- **sudo** — Blocked in all forms: direct, piped, chained, subshell
- **su** — Blocked entirely

#### Disk Formatting & Partitioning
- **mkfs** (all variants), **fdisk / gdisk / cfdisk / sfdisk / sgdisk**, **parted**, **wipefs**
- **diskutil eraseDisk / eraseVolume / partitionDisk** (macOS)
- **dd to block devices** (`of=/dev/...`)

#### Filesystem Destruction
- **rm -rf /** (all variants including `--no-preserve-root`)
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
Nebo cannot modify or delete its own database, WAL, or data files.

| Platform | Protected Path |
|----------|---------------|
| macOS | `~/Library/Application Support/Nebo/data/` |
| Linux | `~/.config/nebo/data/` |
| Windows | `%APPDATA%\Nebo\data\` |
| Override | `$NEBO_DATA_DIR/data/` |

#### Sensitive User Paths (No Writes)
- `~/.ssh/` — SSH keys and configuration
- `~/.gnupg/` — GPG keys and configuration
- `~/.aws/credentials` — AWS credentials
- `~/.kube/config` — Kubernetes credentials
- `~/.docker/config.json` — Docker registry credentials

### What To Do If You Need These Operations
The safeguard error messages tell the user to perform the operation manually in a terminal. Nebo is not designed for system administration — it helps with development and productivity tasks within user-space directories.

---

## 2. Origin-Based Tool Restrictions

**File:** `crates/tools/src/policy.rs`

Every request is tagged with an origin that tracks where it came from.

| Origin | Source | Intended Restrictions |
|--------|--------|----------------------|
| `user` | Direct interaction (web UI, CLI) | None (governed by policy) |
| `system` | Internal tasks (heartbeat, cron, recovery) | None |
| `comm` | Inter-agent communication | **Shell denied** |
| `app` | External app binary | **Shell denied** |
| `skill` | Matched skill template | **Shell denied** |

---

## 3. Tool Policy & Approval System

**File:** `crates/tools/src/policy.rs`

### Policy Levels
- **Allowlist** (default) — Only whitelisted commands auto-approve; everything else prompts
- **Deny** — All operations require approval
- **Full** — All operations auto-approve (dangerous, opt-in only)

### Safe Commands (Auto-Approved)
Read-only and inspection commands: `ls`, `pwd`, `cat`, `head`, `tail`, `grep`, `find`, `which`, `jq`, `cut`, `sort`, `uniq`, `wc`, `echo`, `date`, `env`, `git status`, `git log`, `git diff`, `git branch`, `git show`, `go version`, `node --version`, `python --version`

### Dangerous Command Detection
The `is_dangerous()` function flags: `rm -rf`, `rm -r`, `sudo`, `su`, `chmod 777`, `chown`, `dd`, `mkfs`, `curl | sh`, `wget | sh`, `eval`, `exec`, fork bombs.

### Autonomous Mode
When enabled, bypasses all approval prompts. When disabled, **all permissions reset to defaults** (only Chat & Memory enabled).

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

Disabled categories prevent the tool from being registered with the agent entirely — the LLM never sees it as an available tool. Enforced in `Registry::register_all_with_permissions()` which filters tool registration by category map.

---

## 5. App Sandbox Security

Third-party apps (`.napp` packages) run in a sandboxed environment.

### Environment Isolation (`crates/napp/src/sandbox.rs`)
- **Allowlist-only environment**: Only `PATH`, `HOME`, `TMPDIR`, `LANG`, `LC_ALL`, `TZ`, and `NEBO_APP_*` variables are passed to apps
- All parent environment variables (API keys, secrets, tokens) are **stripped**

### Binary Validation
- Rejects symlinks (path traversal prevention)
- Rejects non-regular files (devices, pipes)
- Checks executable bit
- Enforces size limit (500MB default)

### App Signing (`crates/napp/src/signing.rs`)
- **ED25519** signature verification over raw bytes
- Manifest signature: verifies manifest.json integrity
- Binary signature: SHA256 integrity check + ED25519 verification
- Key ID rotation detection (`SHA256(publicKey)[:8]`)

### Deny-by-Default Permissions (`crates/napp/src/manifest.rs`)
Apps must declare every permission they need. Unknown permissions are rejected. Categories include:
- `network:*` (DNS, HTTP, WebSocket)
- `filesystem:read`, `filesystem:write`
- `shell:execute`, `shell:background`
- `memory:read`, `memory:write`
- `session:*`, `tool:*`, `model:*`, `channel:*`, `mcp:*`

---

## 6. Compiled-Only Binary Policy (No Interpreted Languages)

Nebo's app platform **exclusively runs compiled native binaries**. Apps written in interpreted or scripted languages are rejected.

### Why

Nebo is an AI agent platform. The agent has file system access. If an app's source code is accessible at runtime, the agent can modify it. An interpreted app exposes its entire logic as readable, modifiable plaintext.

### Supported Languages

| Language | Status |
|----------|--------|
| Go | **Recommended** |
| Rust | Supported |
| C / C++ | Supported |
| Zig | Supported |

### Rejected Languages

| Language | Reason |
|----------|--------|
| Node.js / JavaScript | Source code is plaintext `.js` files |
| Python | Source code is plaintext `.py` files |
| Ruby, PHP, Perl | Source code is plaintext |
| Shell scripts | Direct command injection vector |
| Java / Kotlin (JVM) | Decompilable bytecode, requires runtime |
| .NET / C# (Mono) | Decompilable IL, requires runtime |

### Enforcement

**NeboLoop (distribution-time):** Magic byte verification, hidden interpreter detection, dynamic link analysis, Go build info verification, dropper pattern detection, ED25519 signing.

**Nebo (install/launch-time):** Magic byte check, shebang rejection, ED25519 signature verify, file permission check.

---

## 7. Network Security

### Authentication
- **JWT** (HMAC-SHA256) for all API requests (`crates/auth/src/jwt.rs`)
- **WebSocket authentication required** — unauthenticated connections rejected
- **Password hashing**: bcrypt with cost factor 12

### Security Headers (`crates/server/src/middleware.rs`)

**All routes:**
- **Permissions-Policy**: `accelerometer=(), camera=(self), geolocation=(), gyroscope=(), magnetometer=(), microphone=(self), payment=(), usb=()`
- **Strict-Transport-Security**: `max-age=31536000; includeSubDomains; preload`
- **X-Frame-Options**: `DENY`
- **X-Content-Type-Options**: `nosniff`
- **X-XSS-Protection**: `1; mode=block`
- **Referrer-Policy**: `strict-origin-when-cross-origin`

**API routes (`/api/v1/*`) only:**
- **Content-Security-Policy**: `default-src 'none'; frame-ancestors 'none'` — blocks all content loading in API responses
- **Cache-Control**: `no-store, no-cache, must-revalidate, private`
- **Pragma**: `no-cache`

SPA routes intentionally omit CSP to allow embedded content (YouTube, Vimeo, Twitter, external fonts). The SPA is static files served from localhost — the attack surface is the API, not the frontend shell.

### CORS (`crates/server/src/lib.rs`)
- Localhost-only whitelist (ports 27895, 5173, 4173)
- Not `*` — explicit origin validation

### Rate Limiting (`crates/server/src/middleware.rs`)
- Token bucket algorithm, per-client IP
- Auth endpoints: 10 requests/minute
- Uses `RemoteAddr` only — does not trust `X-Forwarded-For`

### Encryption at Rest
- **All credentials** encrypted with AES-256-GCM (`crates/auth/src/credential.rs`)
- **MCP tokens**: AES-256-GCM with random nonces (`crates/mcp/src/crypto.rs`)
- Master encryption key stored in OS keychain via `keyring` crate (`crates/auth/src/keyring.rs`)
- Fallback: key file with `0600` permissions if keychain unavailable

---

## 8. Process Safety

### Single Instance Lock
Only one Nebo instance per computer. Filesystem locks with PID tracking.

### WebSocket Limits
- Maximum message size: 32KB
- Ping/pong keepalive
- Buffered broadcast channel (256 messages)
- Graceful close on connection drop

### Cancellation
Agent runs use `tokio_util::sync::CancellationToken` for graceful cancellation — never `JoinHandle::abort()`.

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
5. **Unconditional safety** — Critical protections cannot be overridden by any setting, mode, or configuration
