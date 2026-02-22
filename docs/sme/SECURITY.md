# Nebo Security Architecture & Audit Reference

> **Purpose:** This document makes any Claude Code session an immediate security SME for the Nebo project. Read this file on launch to restore full security context.

---

## Threat Model

Nebo is a **desktop application** that runs locally. The primary attack surfaces are:

| Surface | Description | Trust Level |
|---------|-------------|-------------|
| **Local HTTP API** | chi router on `localhost:27895` | Trusted (localhost-only, JWT-gated) |
| **WebSocket (client hub)** | Browser ↔ Nebo realtime events | Trusted (localhost origin + JWT) |
| **WebSocket (agent hub)** | Agent ↔ Server internal connection | Internal (single connection, reconnect=drop) |
| **NeboLoop Comms** | `wss://comms.neboloop.com/ws` | Semi-trusted (JWT auth, bot_id binding) |
| **App Platform (.napp)** | Sandboxed binaries via gRPC | Untrusted (should be signed + sandboxed) |
| **Shell Tool** | Agent executes shell commands | Dangerous (policy-gated, safeguard.go) |
| **File Tool** | Agent reads/writes filesystem | Dangerous (sensitive path blocklist) |
| **OAuth Callbacks** | NeboLoop OAuth redirect flow | External (XSS risk in error rendering) |
| **Updater** | Self-update binary replacement | Critical (integrity verification needed) |

### Key Architectural Security Properties

1. **Single agent paradigm** — ONE WebSocket connection enforced by hub (reconnect drops old). No multi-agent confusion.
2. **Lane-based concurrency** — Work queued through lanes with concurrency limits. Prevents unbounded parallelism.
3. **Origin-based restrictions** — Context carries origin (User/Comm/App/Skill/System). Tools can be denied per-origin. **Currently disabled** (see Finding F-02).
4. **Deny-by-default app permissions** — Apps declare permissions in manifest; anything not declared is blocked.
5. **ED25519 app signing** — Signatures over raw bytes (not hashes). Separate `signatures.json` file. **Verification not wired into install flow** (see Finding F-07).

---

## Security Controls Inventory

### Authentication & Authorization

| Control | File | Status |
|---------|------|--------|
| JWT (HS256) | `internal/middleware/jwt.go` | Active |
| JWT secret generation | `internal/local/settings.go:94-101` | 32-byte crypto/rand, stored in data dir with 0600 perms |
| Token expiry | `internal/local/settings.go:24-25` | 30 days (access + refresh both 2592000s) |
| CSRF protection | `internal/middleware/csrf.go` | Active (double-submit cookie, skips `/api/` paths) |
| CORS | `internal/middleware/cors.go` | Localhost-only via `IsLocalhostOrigin()` |
| WebSocket auth | `internal/websocket/handler.go:27-46` | JWT from cookie/header required |
| Password hashing | `internal/logic/auth/` | bcrypt (standard) |

### Encryption

| Control | File | Status |
|---------|------|--------|
| API key encryption at rest | `internal/mcp/client/crypto.go` | AES-256-GCM, key from OS keychain |
| Credential wrapper | `internal/mcp/client/credential.go` | `Encrypt()`/`Decrypt()` wrappers used in handlers |
| Key storage hierarchy | `crypto.go:21-98` | 1) `NEBO_ENCRYPTION_KEY` env, 2) OS keychain, 3) `JWT_SECRET` derivation, 4) file fallback |
| SQLite WAL mode | `internal/db/sqlite.go:27` | Crash-safe writes |

### App Sandbox

| Control | File | Status |
|---------|------|--------|
| Binary validation | `internal/apps/sandbox.go:74-108` | Rejects scripts, symlinks, checks ELF/Mach-O/PE headers |
| Env sanitization | `internal/apps/sandbox.go:33-69` | Blocks LD_PRELOAD, DYLD_, credentials in env |
| Process isolation | `internal/apps/runtime.go` | Separate process group, signal isolation |
| gRPC communication | `internal/apps/adapter.go` | Unix socket per app, no network exposure |
| Quarantine system | `internal/apps/registry.go:589+` | `.quarantined` marker file blocks launch |
| Log isolation | `internal/apps/sandbox.go` | Per-app log directories |

### Shell Safety

| Control | File | Status |
|---------|------|--------|
| Command safeguards | `internal/agent/tools/safeguard.go` | Blocks rm -rf /, disk formatting, etc. |
| Tool policy | `internal/agent/tools/policy.go` | Allowlist/deny/full modes with ask-on-miss |
| Safe bins list | `policy.go` | ls, pwd, cat, grep, find, git status/log/diff always allowed |
| Origin restrictions | `internal/agent/tools/origin.go` | Context-propagated origin checks (currently disabled) |

### File Safety

| Control | File | Status |
|---------|------|--------|
| Sensitive path blocklist | `internal/agent/tools/file_tool.go:568-637` | Blocks ~/.ssh, ~/.aws, /etc/shadow, etc. |
| Symlink resolution | `file_tool.go:613-637` | EvalSymlinks before validation (has race condition) |
| .napp extraction | `internal/apps/napp.go` | Zip-slip + symlink protection |

---

## Known Vulnerabilities & Risk Register

### Critical

**F-01: Exposed secrets in .env committed to git history**
- **Location:** `.env` file (gitignored but previously committed)
- **Secrets exposed:** `ACCESS_SECRET`, `ADMIN_PASSWORD`, `ELEVENLABS_API_KEY`
- **Remediation:** Revoke all exposed credentials. Use `git filter-branch` or BFG to scrub history. Add `git-secrets` pre-commit hook.
- **Status:** OPEN

**F-07: No signature verification on app install**
- **Location:** `internal/apps/install.go:52-79` (`handleInstall`)
- **Issue:** `DownloadAndExtractNapp()` is called but `VerifyAppSignatures()` is never invoked before `launchAndRegister()`.
- **Impact:** Tampered app binaries served from compromised download URLs will be installed and launched without cryptographic validation.
- **Remediation:** Call `VerifyAppSignatures()` after extraction, before launch.
- **Status:** OPEN

**F-08: No signature verification on app update**
- **Location:** `internal/apps/install.go:81-167` (`handleUpdate`)
- **Issue:** Same as F-07 but for the update path. Downloads replacement binary without signature check.
- **Remediation:** Verify signatures on temp directory before swapping into place.
- **Status:** OPEN

**F-09: Revocation check failures are non-fatal**
- **Location:** `internal/apps/runtime.go:155-163`
- **Issue:** When `revChecker.IsRevoked()` returns an error (network failure), the code logs a warning and proceeds to launch the app.
- **Impact:** Revoked apps can run during network outages. Attacker can DoS the revocation endpoint to bypass revocation.
- **Remediation:** Fail closed — refuse to launch if revocation check cannot complete.
- **Status:** OPEN

**F-10: TOCTOU race in InstallFromURL**
- **Location:** `internal/apps/registry.go:159-192`
- **Issue:** Concurrent installs for the same app ID can race between existence check and `os.Rename()`, allowing an attacker's version to win.
- **Remediation:** Serialize installs per app ID using `appLaunchMutex`.
- **Status:** OPEN

**F-18: NeboLoop JWT sub claim extracted without signature verification**
- **Location:** `internal/agent/comm/neboloop/plugin.go:977-996` (`jwtSubClaim`)
- **Issue:** The `ownerID` is derived by base64-decoding the JWT payload without verifying the signature. This value gates `IsOwner` on DM messages (line 466), which routes owner DMs to the main lane with `OriginUser` privileges.
- **Impact:** A forged JWT in a DM could claim owner status and get routed to the main lane, bypassing comm-origin restrictions.
- **Remediation:** Verify JWT signature using NeboLoop's public key before trusting claims. At minimum, compare against the token we sent during CONNECT (which we do trust).
- **Status:** OPEN

**F-19: WebSocket userId query param fallback bypasses JWT auth**
- **Location:** `internal/websocket/handler.go:37-38`
- **Issue:** If JWT parsing fails, code falls back to `r.URL.Query().Get("userId")` — any local process can connect as any user by passing `?userId=xxx`.
- **Impact:** On localhost this is lower severity, but any browser tab or local process can impersonate the owner.
- **Remediation:** Remove the query param fallback entirely. JWT-only auth for WebSocket upgrade.
- **Status:** OPEN (marked as TODO in code)

**F-22: parseJWTClaims() does not verify signature**
- **Location:** `internal/middleware/jwt.go:182-202`
- **Issue:** The `parseJWTClaims()` function splits the JWT on `.`, base64-decodes the payload, and returns claims — **without ever verifying the HMAC signature**. This function is used by `ExternalTokenTranslator`, `ExternalJWTMiddleware`, and the WebSocket `extractUserIDFromJWT`. An attacker can craft a JWT with arbitrary claims and it will be accepted.
- **Impact:** Complete authentication bypass. Any local process or XSS payload can forge a JWT with any `userId`/`email` and impersonate any user.
- **Note:** This is the root cause behind F-18 and F-19. The JWT middleware at `internal/middleware/jwt.go:25-66` (JWTMiddleware) does validate signatures using `jwt.Parse()` + HMAC, so routes using JWTMiddleware are safe. The danger is code paths that call `parseJWTClaims()` directly.
- **Remediation:** Replace `parseJWTClaims()` with a function that calls `jwt.Parse()` with the secret. Or delete it and always use the signature-verifying path.
- **Status:** OPEN

**F-23: Refresh token hashToken() is not a cryptographic hash**
- **Location:** `internal/local/auth.go:331-335`
- **Issue:** `hashToken()` does `copy(b, []byte(token))` then `hex.EncodeToString(b)`. This is truncation + hex encoding, not hashing. The "hash" in the database is reversible to the original token (or a truncated prefix of it).
- **Impact:** If the database is compromised, refresh tokens can be directly recovered and used to mint new access tokens. Database read = full account takeover.
- **Remediation:** Use `sha256.Sum256([]byte(token))` or bcrypt.
- **Status:** OPEN

### High

**F-02: Origin-based tool restrictions disabled**
- **Location:** `internal/agent/tools/policy.go:60-75`
- **Issue:** `defaultOriginDenyList()` returns `nil`. All origins (Comm, App, Skill) can execute shell commands.
- **Impact:** Compromised NeboLoop bots, rogue apps, or malicious skill templates can run arbitrary shell commands.
- **Remediation:** Re-enable the commented-out deny list.
- **Status:** OPEN (intentionally deferred, TODO in code)

**F-03: Reflected XSS in OAuth callback**
- **Location:** `internal/handler/neboloop/oauth.go` (serveCallbackHTML function)
- **Issue:** OAuth `error` query parameter injected into HTML response without escaping.
- **Attack:** `?error=<script>alert('xss')</script>` executes in user's browser.
- **Remediation:** Use `html.EscapeString()` on all user-controlled values in HTML responses.
- **Status:** OPEN

**F-04: Signing key fetch lacks HTTPS enforcement**
- **Location:** `internal/apps/signing.go:69-102`
- **Issue:** `signingClient.Get(url)` uses whatever scheme is in `neboloopURL`. If HTTP, MITM can serve fake public key.
- **Remediation:** Validate `strings.HasPrefix(url, "https://")` before fetching.
- **Status:** OPEN

**F-11: Weak encryption key derivation from JWT_SECRET**
- **Location:** `internal/mcp/client/crypto.go:44-49`
- **Issue:** `copy(key, []byte(secret))` truncates or zero-pads instead of using a proper KDF.
- **Remediation:** Use PBKDF2 or HKDF for key derivation.
- **Status:** OPEN

**F-12: API key info disclosure in error responses**
- **Location:** `internal/handler/provider/testauthprofilehandler.go:113,154,195`
- **Issue:** Raw API error responses forwarded to frontend clients.
- **Remediation:** Log full errors server-side, return generic message to client.
- **Status:** OPEN

**F-13: Google API key in URL query parameter**
- **Location:** `internal/handler/provider/testauthprofilehandler.go:169`
- **Issue:** `?key=<api_key>` in URL exposes key in logs, referrer headers, proxy logs.
- **Remediation:** Use `x-goog-api-key` header instead.
- **Status:** OPEN

**F-20: Agent hub WebSocket has no message size rate limiting**
- **Location:** `internal/agenthub/hub.go:458`
- **Issue:** `SetReadLimit(10 * 1024 * 1024)` allows 10MB messages. No per-connection rate limiting.
- **Impact:** Local DoS via rapid large messages exhausting memory. Lower severity since agent hub is internal, but a compromised agent process could abuse this.
- **Remediation:** Reduce to 1-2MB. Add message rate limiting.
- **Status:** OPEN

### Medium

**F-21: Lane event queue unbounded (DoS vector)**
- **Location:** `internal/agenthub/lane.go:42-57`
- **Issue:** `LaneEvents` has concurrency `0` (unlimited). No queue depth cap. `EnqueueAsync()` spawns a goroutine per task.
- **Impact:** A flood of events (cron, scheduled, triggered) can exhaust memory/goroutines.
- **Remediation:** Set finite default for events lane. Add queue depth monitoring and hard cap.
- **Status:** OPEN

**F-05: Symlink race condition in file tool**
- **Location:** `internal/agent/tools/file_tool.go:613-637`
- **Issue:** For write operations on new files, `EvalSymlinks` fails (file doesn't exist), validation passes on `absPath` only. Between validation and write, attacker can create symlink to sensitive path.
- **Remediation:** Use `os.Lstat()` on parent directory. Reject symlinks completely for write operations.
- **Status:** OPEN

**F-06: Revocation cache poisoning / stale data**
- **Location:** `internal/apps/signing.go:252-291`
- **Issue:** On first fetch failure, `rc.revoked[appID]` returns `false` (not revoked) because the map is empty. Optimistically unsafe.
- **Remediation:** Track `isReady` state. Don't serve from cache until first successful fetch.
- **Status:** OPEN

**F-14: Database directory permissions too permissive**
- **Location:** `internal/db/sqlite.go:21`
- **Issue:** `os.MkdirAll(dir, 0755)` allows other local users to traverse into DB directory.
- **Remediation:** Change to `0700`.
- **Status:** OPEN

**F-15: Revocation cache TTL too long (1 hour)**
- **Location:** `internal/apps/signing.go:229`
- **Issue:** Revoked apps continue running for up to 1 hour after global revocation.
- **Remediation:** Reduce TTL to 5-15 minutes. Consider out-of-band revocation via NeboLoop comms.
- **Status:** OPEN

### Low

**F-16: generateSecret() PID fallback**
- **Location:** `internal/local/settings.go:97-98`
- **Issue:** If `crypto/rand.Read()` fails, falls back to `nebo-<PID>` which is predictable.
- **Remediation:** Panic instead of falling back to weak secret.
- **Status:** OPEN

**F-17: Sensitive path list fails open when UserHomeDir errors**
- **Location:** `internal/agent/tools/file_tool.go:568-608`
- **Issue:** `home, _ := os.UserHomeDir()` ignores error. If home is empty string, all home-relative paths become ineffective.
- **Remediation:** Log and use safe fallback, or refuse to operate.
- **Status:** OPEN

---

## Security Architecture Decisions

### Why localhost-only is acceptable
Nebo binds to `localhost:27895`. The WebSocket upgrader and CORS middleware enforce `IsLocalhostOrigin()`. This means:
- No remote network attack surface on the HTTP/WS layer
- XSS on localhost pages could still exploit the API (hence CSRF protection)
- The real remote attack surface is NeboLoop comms (WebSocket gateway)

### Why JWT HS256 is acceptable
For a single-user desktop app, HS256 with a 256-bit random secret is sufficient. RS256 would add complexity without security benefit since there's no multi-party token verification.

### Why 30-day token expiry
Desktop app — users shouldn't have to re-authenticate frequently. The JWT secret is per-installation and stored with 0600 permissions.

### App signing verification gap
The ED25519 signing infrastructure (`signing.go`) is complete and correct. The gap is that `install.go` and `registry.go` don't call the verification functions. This is the highest-priority security fix for the app platform.

---

## File Quick Reference for Security Auditing

| Area | Key Files |
|------|-----------|
| **Auth** | `internal/middleware/jwt.go`, `internal/local/settings.go`, `internal/middleware/csrf.go` |
| **Encryption** | `internal/mcp/client/crypto.go`, `internal/mcp/client/credential.go` |
| **App signing** | `internal/apps/signing.go` (ED25519 verify + revocation checker) |
| **App sandbox** | `internal/apps/sandbox.go`, `internal/apps/runtime.go`, `internal/apps/napp.go` |
| **App install** | `internal/apps/install.go`, `internal/apps/registry.go` (InstallFromURL) |
| **Shell safety** | `internal/agent/tools/safeguard.go`, `internal/agent/tools/policy.go` |
| **File safety** | `internal/agent/tools/file_tool.go` (validateFilePath, sensitivePaths) |
| **Origin system** | `internal/agent/tools/origin.go` (WithOrigin/GetOrigin context propagation) |
| **WebSocket auth** | `internal/websocket/handler.go`, `internal/middleware/cors.go` |
| **CORS** | `internal/middleware/cors.go` (IsLocalhostOrigin) |
| **OAuth** | `internal/handler/neboloop/oauth.go` (XSS in serveCallbackHTML) |
| **Provider test** | `internal/handler/provider/testauthprofilehandler.go` (info disclosure) |
| **Updater** | `internal/updater/updater.go`, `internal/updater/apply_unix.go` |
| **NeboLoop comms** | `internal/neboloop/sdk/client.go`, `internal/agent/comm/neboloop/plugin.go` |

---

## Remediation Priority Queue

**Do first (blocks launch):**
1. F-22: Fix `parseJWTClaims()` to verify signature — root cause for multiple auth bypasses
2. F-23: Replace `hashToken()` with real SHA-256 hash
3. F-03: XSS in OAuth callback — one-line fix with `html.EscapeString()`
4. F-07/F-08: Wire signature verification into install + update paths
5. F-09: Make revocation check fail-closed
6. F-01: Scrub secrets from git history, rotate credentials
7. F-18: Verify NeboLoop JWT signature before trusting sub claim
8. F-19: Remove WebSocket userId query param fallback

**Do before public release:**
7. F-02: Re-enable origin-based tool restrictions
8. F-04: Enforce HTTPS for signing key + revocation fetches
9. F-10: Serialize installs per app ID
10. F-11: Use proper KDF for encryption key derivation
11. F-12: Sanitize API error responses
12. F-14: Fix database directory permissions
13. F-20: Reduce agent hub message size limit

**Backlog:**
14. F-05: Symlink race condition hardening
15. F-06: Revocation cache readiness tracking
16. F-13: Move Google API key from URL to header
17. F-15: Reduce revocation cache TTL
18. F-16/F-17: Edge case hardening
19. F-21: Lane event queue depth cap

---

## Ongoing Security Practices

- **Pre-commit:** Add `git-secrets` or `detect-secrets` to prevent credential commits
- **Dependency audit:** `go mod audit` + `pnpm audit` on CI
- **Code review checklist:** No hardcoded secrets, encrypted API keys, sanitized error responses, correct file permissions
- **Testing:** `safeguard_test.go` covers shell command blocking — extend for new blocked patterns
- **Monitoring:** NeboLoop comms connection failures may indicate MITM — log and alert
