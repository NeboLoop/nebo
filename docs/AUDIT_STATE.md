# Documentation Audit: State of Where We Are

Audited: 2026-02-12
Documents: README.md, SECURITY.md, docs/CREATING_APPS.md
Method: Cross-referenced every claim against the actual codebase

---

## Executive Summary

README.md is in good shape after the recent rewrite. SECURITY.md is technically thorough but has one **critical discrepancy** (WebSocket origin validation marked CLOSED but not actually implemented). CREATING_APPS.md has the most issues — SDK packages don't exist yet, proto version references are wrong, and MQTT topics don't match the code.

---

## 1. README.md — Accuracy: HIGH

| Claim | Status | Notes |
|-------|--------|-------|
| Anthropic (streaming, tool calls, extended thinking) | VERIFIED | api_anthropic.go, ThinkingConfig support |
| OpenAI (streaming, tool calls) | VERIFIED | api_openai.go |
| Google Gemini (streaming, tool calls) | VERIFIED | api_gemini.go |
| Ollama (local models, no API key) | VERIFIED | api_ollama.go |
| CLI wrappers (claude, gemini, codex) | VERIFIED | cli_provider.go |
| File domain (read, write, edit, search) | VERIFIED | file_tool.go |
| Shell domain (execute, manage, background) | VERIFIED | shell_tool.go |
| Web domain (fetch, search, browser automation) | VERIFIED | web_tool.go |
| Memory (store and recall) | VERIFIED | memory.go |
| Tasks (sub-agents, scheduled jobs) | VERIFIED | orchestrator/, agent_tool.go |
| Communication (inter-agent messaging) | OVERSTATED | Works via MQTT bridge, not a generic comm API |
| Channels (Telegram, Discord, Slack) | OVERSTATED | Proxied through NeboLoop, not native to Nebo |
| Desktop mode (Wails v3, system tray) | VERIFIED | desktop.go, Wails v3 alpha.67 |
| `nebo doctor` | VERIFIED | Full diagnostics in cmd/nebo/doctor.go |
| `nebo session list` | VERIFIED | cmd/nebo/session.go |
| models.yaml configuration | VERIFIED | Loaded in internal/agent/config/ |
| Ollama auto-pull (~4 GB) | VERIFIED | EnsureOllamaModel() exists |
| macOS 13+ / Windows 10+ / Ubuntu 22.04+ | CLAIMED | Not enforced at runtime |
| App platform (sandbox, signing, NeboLoop) | VERIFIED | Full implementation in internal/apps/ |
| License (ELv2 core, Apache 2.0 SDK) | VERIFIED | LICENSE file matches |

### Issues to Fix

1. **Channels section is misleading.** README says "Create bot via @BotFather, add token in Settings" implying native integration. Reality: NeboLoop runs the Telegram/Discord/Slack bridges, Nebo receives messages via MQTT. Users need a NeboLoop account for channels.

2. **Communication row vague.** "Inter-agent messaging via comm plugins" doesn't tell the user anything useful. Consider rewording or removing.

---

## 2. SECURITY.md — Accuracy: HIGH (with one critical issue)

### Verified (working as documented)

| Feature | File | Status |
|---------|------|--------|
| 7 hard safeguards (sudo, su, mkfs, dd, rm -rf /, fork bomb, protected paths) | safeguard.go | VERIFIED |
| 5 origin types (User, Comm, App, Skill, System) | origin.go | VERIFIED |
| 3 policy levels (Deny, Allowlist, Full) | policy.go | VERIFIED |
| 3 approval modes (Off, OnMiss, Always) | policy.go | VERIFIED |
| Origin-based deny lists (comm/app/skill blocked from shell) | policy.go | VERIFIED |
| Sandbox env sanitization (allowlist-only) | sandbox.go | VERIFIED |
| Binary validation (magic bytes, size, symlink rejection) | sandbox.go | VERIFIED |
| ED25519 signing verification | signing.go | VERIFIED |
| Signing key cache (24h) + revocation cache (1h) | signing.go | VERIFIED |
| Process group isolation | runtime.go + process_unix.go | VERIFIED |
| Shell injection prevention (sanitizedEnv, 30+ vars stripped) | shell_tool.go | VERIFIED |
| SSRF protection (private IP blocking, redirect validation) | web_tool.go | VERIFIED |
| Path traversal prevention (sensitive path blocklist, symlink resolution) | file_tool.go | VERIFIED |
| X-Forwarded-For spoofing prevention | middleware/ratelimit.go | VERIFIED |
| CORS localhost-only | middleware/cors.go | VERIFIED |
| Dev-login endpoint removed | N/A | VERIFIED (deleted) |

### CRITICAL: WebSocket Origin Validation NOT IMPLEMENTED

**SECURITY.md claims CSWSH (Cross-Site WebSocket Hijacking) is "CLOSED"** with an `IsLocalOrigin()` helper added to `middleware/cors.go`.

**Reality:**
- `internal/agenthub/hub.go` line 85: `CheckOrigin: func(r *http.Request) bool { return true }`
- `internal/websocket/handler.go` line 17: `CheckOrigin: func(r *http.Request) bool { return true }`
- No `IsLocalOrigin()` function exists anywhere in the codebase

**Impact:** Any website a user visits could open a WebSocket connection to localhost:27895 and interact with the agent. The vulnerability is documented as fixed but is NOT fixed.

**Action required:** Either implement the fix or change the status from CLOSED to OPEN.

### Other Issues

| Issue | Status | Details |
|-------|--------|---------|
| Proto version references | OUTDATED | Document references `proto/apps/v1/` — actual is `proto/apps/v0/` |
| Skills signature verification | CORRECTLY DOCUMENTED AS INCOMPLETE | loader.go doesn't verify signatures yet |
| End-user-irrelevant language | NONE FOUND | Document is appropriately technical for its audience |

---

## 3. docs/CREATING_APPS.md — Accuracy: MEDIUM

This document has the most discrepancies. It's a developer guide so accuracy matters for external developers.

### Verified (correct)

| Claim | Status |
|-------|--------|
| 8 UI block types (text, heading, input, button, select, toggle, divider, image) | VERIFIED against ui.proto |
| Manifest required fields (id, name, version, provides) | VERIFIED against manifest.go |
| Environment variables (NEBO_APP_DIR, NEBO_APP_SOCK, NEBO_APP_ID, NEBO_APP_NAME, NEBO_APP_VERSION, NEBO_APP_DATA) | VERIFIED against sandbox.go |
| Schedule capability | VERIFIED (CapSchedule in manifest.go, schedule.proto exists) |
| Core permissions (network, filesystem, memory, session, tool, shell, channel, comm, model, user, schedule, database, storage) | VERIFIED |
| `nebo apps list` CLI command | VERIFIED in cmd/nebo/plugins.go |
| `nebo apps uninstall` CLI command | VERIFIED in cmd/nebo/plugins.go |

### Issues

| Issue | Severity | Details |
|-------|----------|---------|
| **SDK packages don't exist** | **HIGH** | Doc references `github.com/nebolabs/nebo-sdk-go`, Rust `nebo-sdk` crate, and C headers. None of these exist. This is the primary developer on-ramp and it's missing. |
| **Proto version wrong** | MEDIUM | Doc says channel proto is v1 (`proto/apps/v1/channel.proto`). Actual: `proto/apps/v0/channel.proto`. All protos are in v0/. |
| **MQTT topics don't match code** | MEDIUM | Doc says `neboloop/bot/{botID}/channels/{channelType}/inbound`. Code uses `neboloop/bot/{botID}/chat/in` and `chat/out`. |
| **`nebo apps info` doesn't exist** | LOW | Referenced in doc but not implemented as a CLI command. |
| **13 additional permissions undocumented** | LOW | Code has settings, capability, context, subagent, lane, notification, embedding, skill, advisor, mcp, voice, browser, oauth permissions. Doc only covers core 13. |
| **Manifest fields incomplete** | LOW | `signature` and `oauth` fields exist in manifest.go but aren't documented. |
| **developer.neboloop.com** | UNKNOWN | Referenced as "Full API documentation" — may not be live yet. |

---

## Action Items (prioritized)

### Must Fix Before Launch

1. **CSWSH vulnerability** — Either implement `IsLocalOrigin()` WebSocket origin validation or update SECURITY.md to mark it as OPEN. This is a real security issue.

2. **SDK packages** — Either create the SDKs or add a clear note in CREATING_APPS.md that SDKs are coming and show raw gRPC usage in the meantime.

3. **Proto version references** — Change all `v1` references to `v0` across SECURITY.md and CREATING_APPS.md.

### Should Fix

4. **MQTT topics in CREATING_APPS.md** — Update from `channels/{channelType}/inbound` to `chat/in` and `chat/out`.

5. **README channels section** — Clarify that channels are provided by NeboLoop, not native to Nebo.

6. **README communication row** — Reword to be useful to end users or remove.

### Nice to Have

7. **`nebo apps info` command** — Either implement it or remove the reference.

8. **Additional permissions documentation** — Add an "advanced permissions" section or note that more exist.

9. **Runtime version checks** — Consider adding OS version validation to match stated requirements.

---

## Document-by-Document Recommendations

### README.md
- Clarify channels are through NeboLoop
- Reword or remove "Communication" row
- Otherwise solid — no other changes needed

### SECURITY.md
- Fix CSWSH status (CLOSED → OPEN, or implement the fix)
- Update proto version references (v1 → v0)
- No end-user-irrelevant language found — document is appropriately scoped

### CREATING_APPS.md
- Add SDK availability notice (coming soon / use raw gRPC for now)
- Fix proto version references throughout (v1 → v0)
- Update MQTT topic patterns to match code
- Remove `nebo apps info` reference or add the command
- Consider documenting additional permissions
- Document `signature` and `oauth` manifest fields
