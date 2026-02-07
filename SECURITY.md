# NEBO SECURITY GAP ANALYSIS

**Mapping OpenClaw CVEs & Architectural Vulnerabilities Against Nebo's Architecture**

*February 5, 2026 — Based on OpenClaw CVE research and Nebo architecture documentation*

---

## Executive Summary

OpenClaw (formerly Clawdbot/Moltbot) has accumulated 5 CVEs, 7+ GitHub Security Advisories, and severe architectural vulnerabilities within three months of launch. This analysis maps each known vulnerability against Nebo's architecture to determine which risks apply, which are avoided by design, and what must be fixed.

Of 18 identified vulnerability classes, Nebo is protected from 5 by architectural differences (no browser gateway UI, no Chrome extension, no Lobster engine, no SSH remote mode, no Docker sandbox). Nebo shared 8 critical or high-severity vulnerabilities with OpenClaw — 6 of which are now mitigated (memory injection, compaction poisoning, NeboLoop remote access, origin-based tool policies, SSRF, path traversal). Two critical items remain: unsigned plugins and skills supply chain. Four additional items carry moderate risk requiring attention.

| Critical / Vulnerable | At Risk / Partial | Not Applicable | Total Reviewed |
|:---:|:---:|:---:|:---:|
| **8** | **5** | **5** | **18** |

---

## Remediation Progress

*Last updated: February 7, 2026*

### Completed Fixes

| Fix | Date | Description | Files Changed |
|-----|------|-------------|---------------|
| **Dev-login endpoint removed** | 2026-02-05 | Deleted passwordless JWT login endpoint entirely. Existing auth system (register/login/setup wizard) covers all use cases. | `internal/handler/auth/devloginhandler.go` (deleted), `internal/server/server.go`, `app/src/lib/api/nebo.ts`, `app/src/routes/(app)/dev-login/` (deleted) |
| **CORS origins restricted** | 2026-02-05 | `DefaultCORSConfig()` now returns explicit localhost-only origins instead of empty list. Added `BaseURL`-derived fallback for production deployments. Three tiers: explicit config > BaseURL-derived > localhost defaults. | `internal/middleware/cors.go`, `internal/middleware/security.go` |
| **local.nebo.bot purged** | 2026-02-05 | Removed all references to `local.nebo.bot` domain from entire codebase (15+ files) to prevent DNS hijack attacks. All defaults now use `localhost`. | `internal/config/config.go`, `app/vite.config.ts`, `etc/nebo.yaml`, `.env.example`, `internal/browser/relay.go`, `assets/chrome-extension/`, and more |
| **WebSocket CSWSH fixed** | 2026-02-05 | Added `IsLocalOrigin()` helper that validates Origin header against localhost/127.0.0.1. Applied to agenthub and chat WebSocket upgraders. Empty Origin allowed for direct CLI/agent connections. | `internal/middleware/cors.go`, `internal/agenthub/hub.go`, `internal/websocket/handler.go` |
| **X-Forwarded-For spoofing fixed** | 2026-02-05 | `DefaultKeyFunc` now uses only `RemoteAddr`, ignoring spoofable headers. Added `TrustedProxyKeyFunc` for known reverse proxy deployments. 10 tests pass. | `internal/middleware/ratelimit.go`, `internal/middleware/ratelimit_test.go` |
| **SSRF protections added** | 2026-02-05 | Three-layer SSRF defense: (1) `validateFetchURL()` pre-flight blocks private IPs, non-HTTP schemes, metadata endpoints. (2) `ssrfSafeTransport()` custom dialer re-validates at connection time (catches DNS rebinding). (3) `ssrfSafeRedirectCheck()` validates every redirect target. Blocks 127.0.0.0/8, 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 169.254.0.0/16, ::1/128, fc00::/7, fe80::/10, and more. 25 tests pass. | `internal/agent/tools/web_tool.go`, `internal/agent/tools/tools_test.go` |
| **Path traversal protections added** | 2026-02-05 | `validateFilePath()` blocks access to sensitive paths (~/.ssh, ~/.aws, ~/.gnupg, ~/.docker/config.json, ~/.kube/config, ~/.npmrc, ~/.password-store, browser profiles, shell rc files, /etc/shadow, /etc/passwd, /etc/sudoers). Symlink resolution via `filepath.EvalSymlinks` prevents symlink-based traversal. Applied to read, write, and edit handlers. 12 tests pass. | `internal/agent/tools/file_tool.go`, `internal/agent/tools/tools_test.go` |
| **Shell env var sanitization** | 2026-02-05 | `sanitizedEnv()` strips dangerous environment variables before shell execution. Blocks LD_PRELOAD/LD_LIBRARY_PATH/LD_AUDIT (Linux linker injection), DYLD_INSERT_LIBRARIES/DYLD_LIBRARY_PATH/DYLD_FRAMEWORK_PATH (macOS linker injection), IFS/CDPATH/BASH_ENV/PROMPT_COMMAND/SHELLOPTS (shell behavior manipulation), BASH_FUNC_* (ShellShock), PYTHONSTARTUP/NODE_OPTIONS/RUBYOPT/PERL5OPT (interpreter injection). All LD_/DYLD_ prefixes blocked by wildcard. Applied to both foreground (`handleBash`) and background (`SpawnBackgroundProcess`) execution paths. `ShellCommand()` now uses absolute `/bin/bash` path to prevent PATH-based binary substitution. 5 new tests pass. | `internal/agent/tools/shell_tool.go`, `internal/agent/tools/shell_unix.go`, `internal/agent/tools/process_registry.go`, `internal/agent/tools/tools_test.go` |
| **Origin tagging + origin-aware tool policy** | 2026-02-05 | Every request now carries an `Origin` (user, comm, plugin, skill, system) propagated via `context.Context`. `RunRequest.Origin` field tags all 12 entry points: agent WS handler (user/system/comm), cron callbacks (system), intro flows (system), recovery tasks (system), comm handler (comm), CLI chat (user). `Registry.Execute()` checks `Policy.IsDeniedForOrigin()` before any tool runs — hard deny, no approval prompt. Default deny lists block `shell` for comm/plugin/skill origins. User and system origins unrestricted. 5 new tests pass. | `internal/agent/tools/origin.go` (new), `internal/agent/tools/policy.go`, `internal/agent/tools/registry.go`, `internal/agent/runner/runner.go`, `cmd/nebo/agent.go`, `internal/agent/comm/handler.go`, `cmd/nebo/chat.go`, `internal/agent/tools/tools_test.go` |
| **Memory sanitization + tool-driven recall** | 2026-02-07 | Memory content sanitization: `sanitizeMemoryKey()` and `sanitizeMemoryValue()` enforce length limits (128/2048 chars), strip control characters, and block 15+ prompt-injection patterns. Applied to both `store()` and `StoreEntryForUser()` paths. Gated by configurable `memory.sanitize_content` setting (default: true). Switched from prompt-injected memory (50 tacit memories dumped into system prompt every turn) to tool-driven pattern: agent retrieves memories via `agent(resource: memory, action: recall/search)`. Embeddings made optional via `memory.embeddings` config (default: false). 6 new test functions covering key/value sanitization, injection pattern blocking, safe content pass-through, and control char stripping. | `internal/agent/tools/memory.go`, `internal/agent/tools/agent_tool.go`, `internal/agent/config/config.go`, `internal/agent/memory/dbcontext.go`, `internal/defaults/dotnebo/config.yaml`, `cmd/nebo/agent.go`, `internal/agent/tools/tools_test.go` |
| **Compaction summary sanitization** | 2026-02-07 | `sanitizeForSummary()` strips control characters from user content and tool failure output before inclusion in compaction summaries. Applied to `generateSummary()` (user message content) and `CollectToolFailures()` (tool error content). Pre-compaction memory flush verified: runs synchronously before compaction at both call sites, with 45s timeout, dedup tracking, and cheapest-model selection. 1 new test function for summary sanitization. | `internal/agent/runner/compaction.go`, `internal/agent/runner/runner.go`, `internal/agent/runner/compaction_test.go` |

### Deferred Items

| Item | Reason |
|------|--------|
| Web content sanitization before LLM (prompt injection) | Deferred — requires broader prompt injection defense strategy |
| CDP relay auth for loopback | Skipped — requires local code execution which already grants full machine access |

---

## Vulnerability-by-Vulnerability Mapping

### CVE-2026-25253 — Critical (CVSS 8.8) — NOT APPLICABLE

**OpenClaw Issue:** 1-click RCE via gatewayUrl token exfiltration + cross-site WebSocket hijacking.

**Nebo Status: NOT APPLICABLE.** Nebo uses localhost WebSocket IPC between goroutines in the same process — no browser-facing gateway UI, no query-string URL routing.

**Remediation:** No action needed. Architecture avoids this by design.

---

### CVE-2026-24763 — High (CVSS 8.8) — NOT APPLICABLE (different risk)

**OpenClaw Issue:** Docker sandbox command injection via PATH environment variable.

**Nebo Status: NOT APPLICABLE (different risk profile).** Nebo does not use Docker sandboxing. Shell tool executes directly under host OS user. **Shell env sanitization now strips LD_PRELOAD, DYLD_INSERT_LIBRARIES, and 30+ dangerous env vars.** Absolute `/bin/bash` path prevents PATH-based binary substitution.

**Remediation:** No sandbox escape, but also no sandbox. Shell env hardening complete. Add origin-aware shell restrictions per remediation plan.

---

### CVE-2026-25157 — High — NOT APPLICABLE

**OpenClaw Issue:** OS command injection via unsanitized project root path in SSH command construction.

**Nebo Status: NOT APPLICABLE.** No SSH remote mode, no macOS desktop app, no sshNodeCommand equivalent.

**Remediation:** No action needed currently. If SSH features are added, sanitize all path inputs in shell construction.

---

### CVE-2026-22708 — High — VULNERABLE (deferred)

**OpenClaw Issue:** Indirect prompt injection via unsanitized web content fed into LLM context.

**Nebo Status: VULNERABLE.** Web tool fetches external content. Output enters agentic loop as tool result. No sanitization between web content and LLM context. **SSRF protections added** (private IPs blocked), but content sanitization deferred.

**Remediation:** Implement web content sanitizer. Strip hidden text, CSS-hidden elements, instruction-like patterns. Tag web tool output with `Origin=web_fetch` for policy enforcement.

---

### CVE-2026-25475 — Moderate (CVSS 6.5) — MITIGATED

**OpenClaw Issue:** Local file inclusion via MEDIA path extraction — unauthorized file reads.

**Nebo Status: MITIGATED.** `validateFilePath()` blocks access to ~25 sensitive paths (SSH keys, AWS/GCP/Azure credentials, GPG keys, browser profiles, shell rc files, /etc/shadow, etc.). Symlink resolution via `filepath.EvalSymlinks` prevents symlink-based traversal. Applied to read, write, and edit handlers. 12 tests pass.

**Remaining:** Per-origin read scope now possible via origin tagging (items #9-11 complete). Can add file tool restrictions per origin as needed.

---

### GHSA-g55j-c2v4-pjcg — High — AT RISK

**OpenClaw Issue:** Unauthenticated local RCE via WebSocket config.apply endpoint — any local process can modify agent config.

**Nebo Status: AT RISK.** Server on port 27895 with Chi HTTP router. JWT authentication exists on protected endpoints. **WebSocket origin validation now enforced** via `IsLocalOrigin()`.

**Remediation:** Verify all HTTP/WebSocket endpoints require authentication. Session tokens already in use for protected routes.

---

### GHSA-r8g4-86fx-92mq — Moderate — LOW RISK

**OpenClaw Issue:** Local file inclusion via MEDIA path extraction in media parser.

**Nebo Status: MITIGATED.** No media parser. File tool path traversal protections now in place (see CVE-2026-25475 fix).

**Remediation:** Complete. Sensitive path blocklist + symlink resolution applied to all file operations.

---

### GHSA-mr32-vwc2-5j6h — High — NOT APPLICABLE

**OpenClaw Issue:** Credential theft via Chrome extension relay — unvalidated /cdp WebSocket accepting arbitrary Chrome DevTools Protocol commands.

**Nebo Status: NOT APPLICABLE.** Browser relay exists but CDP loopback exploitation requires prior local code execution, which already grants full machine access. WebSocket origin validation added to all upgraders.

**Remediation:** No action needed. Risk accepted.

---

### GHSA-4mhr-g7xj-cg8j — High — NOT APPLICABLE

**OpenClaw Issue:** Arbitrary execution via lobsterPath/cwd injection in workflow engine.

**Nebo Status: NOT APPLICABLE.** No Lobster workflow engine equivalent.

**Remediation:** No action needed. If workflow features are added, validate all path/cwd inputs.

---

### ARCH-001: Memory Injection — Critical — MITIGATED

**OpenClaw Issue:** Memory injection — tool outputs stored as "facts" enter future system prompts as raw prose. Enables persistent prompt injection.

**Nebo Status: MITIGATED.** Three defenses implemented: (1) **Content sanitization** — `sanitizeMemoryKey()` and `sanitizeMemoryValue()` strip control characters, enforce length limits (key: 128, value: 2048 chars), and block 15+ prompt-injection patterns (instruction overrides, system prompt tags, persona manipulation). Applied to both `store()` and `StoreEntryForUser()` paths. (2) **Tool-driven memory** — memories are no longer bulk-injected into the system prompt. Agent retrieves memories on-demand via `agent(resource: memory, action: recall/search)`. (3) **Configurable** — sanitization gated by `memory.sanitize_content` config setting (default: true); embeddings gated by `memory.embeddings` (default: false, incurs API costs).

**Remaining:** Per-origin memory write restrictions (comm/plugin origins could be denied memory store). Structured schema validation for memory entries.

---

### ARCH-002: Compaction Poisoning — High — MITIGATED

**OpenClaw Issue:** Compaction summary poisoning — summaries prepended to system prompt. Poisoned summary persists across sessions.

**Nebo Status: MITIGATED.** Three defenses: (1) **Pre-compaction memory flush** — `maybeRunMemoryFlush()` extracts and persists important memories before compaction discards context. Runs synchronously with 45s timeout, dedup tracking via `ShouldRunMemoryFlush`/`RecordMemoryFlush`. (2) **Summary sanitization** — `sanitizeForSummary()` strips control characters from user content and tool failure output before inclusion in compaction summaries. Applied to `generateSummary()` and `CollectToolFailures()`. (3) **Tool failure preservation** — `EnhancedSummary()` appends capped, normalized tool failures (max 8, max 240 chars each) so the agent retains error awareness post-compaction.

**Remaining:** Provenance tracking on summaries (mark which summary entries came from which origin). Structured state snapshots instead of free-text summaries.

---

### ARCH-003: Skills Supply Chain — High — VULNERABLE

**OpenClaw Issue:** Skills as supply chain vector — adversarial skill files become system prompt content.

**Nebo Status: VULNERABLE.** Matched skills append template text to system prompt. No signing or validation.

**Remediation:** Sign skills, make them data-only, or compile them. At minimum, strip instruction-like content from skill templates.

---

### ARCH-004: Remote Comm with Full Tool Access — Critical — MITIGATED

**OpenClaw Issue:** Remote comm channel with full tool access — allowed full agentic loop from external messages.

**Nebo Status: MITIGATED.** Comm-origin requests are now tagged with `OriginComm` and checked against per-origin deny lists in `Registry.Execute()`. Shell access is denied by default for comm origins. All 3 comm handler entry points tagged. 5 tests pass.

**Remaining:** Expand deny list as needed (e.g., deny `agent(resource: cron)` for comm). Add capability tokens for fine-grained comm permissions.

---

### ARCH-005: Unsigned Plugins — Critical — VULNERABLE

**OpenClaw Issue:** Unsigned, unsandboxed plugin binaries with OS-level access.

**Nebo Status: VULNERABLE.** `LoadAll()` scans plugin directories and loads any executable binary with no signature verification, no sandboxing, and full OS-level access.

**Remediation:** Plugin integrity: install manifest + hash verification, approved plugin allowlist in SQLite, no auto-load. Tag plugin outputs with origin for policy enforcement.

---

### ARCH-006: Plaintext Credentials — High — AT RISK

**OpenClaw Issue:** Plaintext credential storage — API keys, OAuth tokens in plain files.

**Nebo Status: AT RISK.** `models.yaml` stores API keys as plain YAML. SQLite `auth_profiles` table has `api_key TEXT` with no encryption.

**Remediation:** Encrypt credentials at rest. Use OS keychain (macOS Keychain, Windows Credential Manager, libsecret on Linux) or encrypt config values with a user-derived key.

---

### ARCH-007: Localhost Trust Assumption — High — PARTIALLY MITIGATED

**OpenClaw Issue:** Localhost trust assumption — treated 127.0.0.1 requests as owner. Collapsed behind reverse proxies.

**Nebo Status: PARTIALLY MITIGATED.** JWT authentication exists on protected endpoints. **WebSocket origin validation now enforced.** **CORS restricted to explicit localhost origins.** **X-Forwarded-For spoofing fixed** in rate limiter. **local.nebo.bot DNS hijack vector eliminated.** Remaining: ensure all endpoints require auth tokens, not just origin checks.

**Remediation:** Continue authenticating all endpoints. Per-session tokens with file-lock secret for additional hardening.

---

### ARCH-008: Sub-agent Recursion — Moderate — PARTIALLY VULNERABLE

**OpenClaw Issue:** Unbounded sub-agent recursion — can spawn unlimited sub-agents to brute-force past restrictions.

**Nebo Status: PARTIALLY VULNERABLE.** Nested lane cap=3 limits recursion, but subagent lane is unlimited. A comm-origin task can spawn many subagents.

**Remediation:** Cap subagents per session, add per-origin rate limits, max wall clock per comm-origin session.

---

## What Nebo Already Gets Right

Nebo's architecture avoids several of OpenClaw's worst vulnerabilities by design. The single-binary Go process with goroutine-based IPC eliminates the browser-facing gateway UI that enabled OpenClaw's most critical CVE (CVE-2026-25253). There is no Chrome extension distribution, no SSH remote mode, no Docker sandbox to escape from, and no Lobster workflow engine. The lane system with bounded concurrency (nested:3, main:1) provides natural rate limiting. The tool approval system with configurable policy levels is a solid foundation — it just needs the origin dimension added.

---

## Implementation Priority Matrix

| # | Implementation Item | Effort | Impact | Status |
|---|---------------------|--------|--------|--------|
| 1 | Dev-login endpoint removal | Small | Eliminates auth bypass | **DONE** |
| 2 | CORS origin restriction | Small | Blocks cross-origin attacks | **DONE** |
| 3 | local.nebo.bot DNS hijack elimination | Small | Removes DNS-based attack vector | **DONE** |
| 4 | WebSocket origin validation (CSWSH) | Small | Blocks cross-site WS hijacking | **DONE** |
| 5 | X-Forwarded-For rate limit bypass | Small | Prevents rate limit evasion | **DONE** |
| 6 | SSRF protections on web fetch | Medium | Blocks internal network scanning | **DONE** |
| 7 | Path traversal protections in file tools | Medium | Blocks sensitive file access | **DONE** |
| 8 | Shell env var sanitization | Medium | Blocks LD_PRELOAD/PATH injection | **DONE** |
| 9 | Origin tagging on sessions + messages | Medium | Foundation for all policy fixes | **DONE** |
| 10 | Origin-aware tool policy in registry | Medium | Blocks injection consequences | **DONE** |
| 11 | Default-deny dangerous tools for comm/plugin origins | Small | Neutralizes NeboLoop + plugin injection | **DONE** |
| 12 | Memory schema + sanitization | Medium | Eliminates persistent prompt injection | **DONE** |
| 13 | Compaction snapshot hardening | Medium | Prevents session-persistent poisoning | **DONE** |
| 14 | Plugin allowlist by hash + no auto-load | Small | Stops malicious plugin loading | Not started |
| 15 | Skills signing or data-only format | Medium | Closes supply-chain prompt injection | Not started |
| 16 | Credential encryption at rest | Medium | Mitigates infostealer harvesting | Not started |
| 17 | Sub-agent rate limiting + wall clock caps | Small | Prevents brute-force via recursion | Not started |

---

## Conclusion

Nebo avoids OpenClaw's most headline-grabbing CVEs (the 1-click RCE, the Chrome extension theft, the Docker escape) through fundamentally different architectural choices. However, it shares the deeper, harder-to-fix vulnerability classes: memory injection into system prompts, unsandboxed plugin execution, remote comm channels with full tool authority, and the absence of origin-based access control.

Thirteen fixes have been completed — eight infrastructure-level (auth bypass, CORS, DNS hijack, CSWSH, rate limit bypass, SSRF, path traversal, shell env sanitization), three application-layer (origin tagging, origin-aware tool policy, default-deny for non-user origins), and two memory/compaction hardening (memory sanitization with tool-driven recall, compaction summary sanitization). The origin tagging system provides the foundation for all remaining policy fixes. Remaining items address supply chain integrity (plugin signing, skills signing) and credential encryption.

**The key lesson from OpenClaw's experience: these vulnerabilities were discovered and exploited within weeks of the project gaining popularity. The origin-based authority wall and memory injection defenses are now in place — the next priority is supply chain integrity (plugins + skills).**
