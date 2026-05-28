# Nebo — Known Gaps & Incomplete Features

> **Last updated:** 2026-05-15
> **Source:** Comprehensive audit of all 60 systems via SME documentation
> **Total gaps identified:** 50+

---

## Summary

| Severity | Count | Description |
|----------|-------|-------------|
| Critical | 5 | Security holes, missing protections, disabled safety features |
| High | 8 | Broken or disabled user-facing features |
| Medium | 10 | Hardcoded values, missing cleanup, dead code paths |
| Low | 9 | Design debt, minor UX gaps, incomplete platform coverage |

---

## Critical

Issues that affect security, data integrity, or correctness.

### GAP-001: Secret scanning does not cover chat messages or tool outputs

- **System:** Secret Scanning
- **SME Doc:** `SECRET_SCANNING_SME.md`
- **Files:** `crates/agent/src/secret_scan.rs`, `crates/agent/src/memory.rs`
- **Description:** The secret scanner (`detect_secret()`) is only called from `store_facts()` in `memory.rs:231` before memory entries are written to SQLite. Raw chat messages stored via `insert_chat_message()` and tool call results stored in the chat transcript are **never scanned**. A user pasting an API key in chat, or a tool returning credentials in its output, will be stored unredacted in the database.
- **Impact:** Secrets can persist in `chat_messages` table indefinitely without detection.
- **Suggested fix:** Add `detect_secret()` call in `chat_dispatch.rs` before `insert_chat_message()`, and in the tool result handler before appending to transcript.

### GAP-002: Secret detection is silent — no user notification

- **System:** Secret Scanning
- **SME Doc:** `SECRET_SCANNING_SME.md`
- **Files:** `crates/agent/src/secret_scan.rs`, `crates/agent/src/memory.rs`
- **Description:** When a secret is detected in a memory fact, the entry is silently dropped. The user receives no indication that a memory was blocked. The agent also receives no feedback that its extraction was rejected.
- **Impact:** Users may wonder why certain facts aren't remembered. Agents may repeatedly attempt to store the same blocked fact.
- **Suggested fix:** Return a warning in the stream or log a user-visible notification when secrets are detected and blocked.

### GAP-003: Content Security Policy disabled in Tauri

- **System:** Tauri Desktop App
- **SME Doc:** `TAURI_DESKTOP_SME.md`
- **Files:** `src-tauri/tauri.conf.json`
- **Description:** The CSP is set to `null` (disabled). The webview has no Content Security Policy enforcement. Combined with the `neboapp://` custom protocol that serves local files, this expands the attack surface for XSS if any user-controlled content is rendered.
- **Impact:** No protection against script injection in the webview. Any XSS vulnerability has full access to Tauri IPC commands.
- **Suggested fix:** Define a restrictive CSP that allows only known origins. At minimum: `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'`.

### GAP-004: Auth email verification endpoints are stubs

- **System:** Auth System
- **SME Doc:** `AUTH_SYSTEM_SME.md`
- **Files:** `crates/auth/src/service.rs`
- **Description:** The `/auth/verify` and `/auth/resend` handler endpoints exist in the route table but their implementations are incomplete stubs. Email verification is not enforced during registration.
- **Impact:** No email verification means accounts can be created with arbitrary email addresses. In a future multi-user or cloud deployment, this would be a significant auth gap.
- **Suggested fix:** Implement verification token generation, email sending (or delegation to NeboAI), and enforcement on login.

### GAP-005: Notification user preference not enforced

- **System:** Notification System
- **SME Doc:** `NOTIFICATION_SYSTEM_SME.md`
- **Files:** `crates/db/src/queries/notifications.rs`, `crates/server/src/handlers/notification.rs`
- **Description:** The `inapp_notifications` boolean preference is stored in the `user_preferences` table but is **never checked** before creating or delivering notifications. All notifications are created regardless of user preference.
- **Impact:** Users who disable notifications still receive them. The setting is purely cosmetic.
- **Suggested fix:** Check `inapp_notifications` preference in `create_notification()` or at the broadcast point before emitting WebSocket events.

---

## High

Broken or disabled user-facing features that affect functionality.

### GAP-006: Native OS notifications completely disabled

- **System:** Notification System
- **SME Doc:** `NOTIFICATION_SYSTEM_SME.md`
- **Files:** `crates/notify/src/lib.rs`
- **Description:** `notify_crate::send()` is a **no-op** — it logs the notification but does not display it. The function is awaiting migration to `tauri-plugin-notification`. Current implementation issues:
  - Cannot deep-link clicked notifications back to the app
  - No custom icons, sounds, or action buttons
  - On macOS, notifications would appear as "osascript" instead of "Nebo"
- **Impact:** Users receive no OS-level notifications for any agent activity, workflow completions, or urgent alerts. Only in-app notifications work.
- **Suggested fix:** Integrate `tauri-plugin-notification` for native notifications with proper app identity and deep linking.

### GAP-007: I18N — V2 pages use hardcoded English strings

- **System:** Internationalization
- **SME Doc:** `I18N_SYSTEM_SME.md`
- **Files:** `app/src/routes/` (V2 pages), `app/src/lib/i18n/`
- **Description:** Most V2 pages (the current active UI) use hardcoded English strings directly in components instead of `$t('key')` translation calls. Only the marketplace pages and a few older components use the i18n system. The V1 pages had proper i18n wiring but V2 pages were written without it.
- **Impact:** Non-English users see English text throughout most of the app. The 25 translated locales are effectively unused in V2.
- **Suggested fix:** Systematic pass through all V2 routes and components to replace hardcoded strings with `$t()` calls.

### GAP-008: I18N — 39 translation keys missing in all non-English locales

- **System:** Internationalization
- **SME Doc:** `I18N_SYSTEM_SME.md`
- **Files:** `app/src/lib/i18n/locales/`
- **Description:** 39 recently-added translation keys exist in `en.json` but have not been added to any of the 24 non-English locale files. These keys fall back to English via svelte-i18n's fallback chain.
- **Impact:** Even in well-translated locales, ~3% of strings display in English.
- **Suggested fix:** Run a diff script to identify missing keys and either machine-translate or flag for human translation.

### GAP-009: I18N — V2 does not sync saved language from backend

- **System:** Internationalization
- **SME Doc:** `I18N_SYSTEM_SME.md`
- **Files:** `app/src/routes/+layout.svelte`, `app/src/lib/i18n/`
- **Description:** V2 reads the locale only from `localStorage` on boot. It does not fetch the user's saved language preference from the backend API (`GET /api/v1/user/me/preferences`). If a user sets their language on one device, it won't carry over to another device or a fresh browser.
- **Impact:** Language preference is device-local only. No cross-device sync.
- **Suggested fix:** Fetch user preferences on boot and sync `localStorage` with the backend-saved locale.

### GAP-010: Desktop input actions not fully implemented

- **System:** Desktop Tools
- **SME Doc:** `DESKTOP_TOOLS_SME.md`
- **Files:** `crates/tools/src/desktop_tool.rs`
- **Description:** The `click`, `get_value`, and `set_value` input actions explicitly return "not yet fully implemented" errors. These are advertised in the tool schema but cannot be used.
- **Impact:** Agents cannot programmatically click UI elements or read/set form values. The "see-then-click" automation loop is incomplete.
- **Suggested fix:** Implement platform-specific input simulation for click (AX API on macOS, xdotool on Linux, UI Automation on Windows) and value get/set via accessibility APIs.

### GAP-011: Desktop macOS maximize uses hardcoded dimensions

- **System:** Desktop Tools
- **SME Doc:** `DESKTOP_TOOLS_SME.md`
- **Files:** `crates/tools/src/desktop_tool.rs`
- **Description:** The macOS window maximize action sets a hardcoded size of 1920x1055 at position {0, 25} instead of using the native maximize API or querying the actual screen dimensions.
- **Impact:** On non-1080p displays (4K, ultrawide, smaller laptops), maximize produces incorrect window sizing.
- **Suggested fix:** Use `NSScreen.mainScreen.visibleFrame` via AppleScript/JXA to get actual screen dimensions.

### GAP-012: Frontend capability toggles forced ON

- **System:** Frontend Settings
- **SME Doc:** `FRONTEND_SETTINGS_SME.md`
- **Files:** `app/src/routes/settings/permissions/+page.svelte`
- **Description:** All 8 capability toggles in the Permissions settings page are forced to ON and disabled (not user-configurable). The permission system exists in the backend but the frontend doesn't allow users to restrict capabilities.
- **Impact:** Users cannot limit what their agents are allowed to do. The permission UI is cosmetic only.
- **Suggested fix:** Wire toggle state to the backend permission API and allow users to restrict specific capabilities.

### GAP-013: Language picker missing from V2 profile settings

- **System:** Frontend Settings
- **SME Doc:** `FRONTEND_SETTINGS_SME.md`
- **Files:** `app/src/routes/settings/profile/+page.svelte`
- **Description:** The V2 profile settings page does not include a language/locale picker. Users have no UI path to change their language preference in V2.
- **Impact:** Users cannot change language without manually editing localStorage.
- **Suggested fix:** Add a locale selector dropdown to the profile settings page, wired to both localStorage and the backend preferences API.

---

## Medium

Hardcoded values, missing scheduled tasks, dead code paths.

### GAP-014: Embedding models hardcoded with no runtime configuration

- **System:** Embedding System
- **SME Doc:** `EMBEDDING_SYSTEM_SME.md`
- **Files:** `crates/ai/src/embedding.rs`
- **Description:** Embedding provider models are hardcoded:
  - OpenAI: `text-embedding-3-small` (1536 dimensions)
  - Ollama: `nomic-embed-text` (768 dimensions)
  - Retry delays: `[500, 2000, 8000]` ms
  - Session FTS dampening: `0.6` weight

  There is no `models.yaml` or settings.json option to change the embedding model at runtime.
- **Impact:** Cannot switch to a better/cheaper embedding model without code changes. Cannot tune search weights without recompilation.
- **Suggested fix:** Add embedding model configuration to `models.yaml` under an `embeddings` section.

### GAP-015: Old notifications accumulate indefinitely

- **System:** Notification System
- **SME Doc:** `NOTIFICATION_SYSTEM_SME.md`
- **Files:** `crates/db/src/queries/notifications.rs`, `crates/server/src/scheduler.rs`
- **Description:** `delete_old_notifications(days: i64)` exists in the DB query layer but is **never called** from the scheduler or any background task. Notifications accumulate in the database with no cleanup.
- **Impact:** Over time, the notifications table grows unbounded. Users with long-running installations will see degraded query performance.
- **Suggested fix:** Add a daily cleanup job in `scheduler.rs` calling `delete_old_notifications(30)`.

### GAP-016: Notification pagination hardcoded

- **System:** Notification System
- **SME Doc:** `NOTIFICATION_SYSTEM_SME.md`
- **Files:** `crates/server/src/handlers/notification.rs`
- **Description:** `list_notifications` uses hardcoded `page_size=50` and `page_offset=0` with no query parameters exposed to the frontend. Users cannot paginate or load more notifications.
- **Impact:** Only the 50 most recent notifications are ever visible.
- **Suggested fix:** Accept `page` and `page_size` query parameters in the handler.

### GAP-017: Database migration sequence has gaps

- **System:** Database Layer
- **SME Doc:** `DATABASE_LAYER_SME.md`
- **Files:** `crates/db/migrations/`
- **Description:** The 92 migrations span 0001-0092 but numbers 0002, 0068, and 0076 are missing from the sequence. The migration runner handles this gracefully (it tracks applied migrations by number, not sequence), but it indicates migrations were deleted or renumbered during development.
- **Impact:** Cosmetic — no functional impact. May confuse contributors auditing the migration history.
- **Suggested fix:** Document the gaps in a comment in the migration runner. No code change needed.

### GAP-018: Task cleanup functions have no scheduled caller

- **System:** Database Layer
- **SME Doc:** `DATABASE_LAYER_SME.md`
- **Files:** `crates/db/src/queries/tasks.rs`, `crates/server/src/scheduler.rs`
- **Description:** `delete_completed_tasks()` and `cleanup_old_task_lists()` are implemented in the query layer but never invoked by the scheduler or any background job.
- **Impact:** Completed tasks accumulate in the database indefinitely.
- **Suggested fix:** Add a periodic cleanup job to the scheduler.

### GAP-019: Secret scanning patterns overlap (OpenAI vs Stripe)

- **System:** Secret Scanning
- **SME Doc:** `SECRET_SCANNING_SME.md`
- **Files:** `crates/agent/src/secret_scan.rs`
- **Description:** Both the OpenAI key pattern and Stripe key pattern match the `sk-` prefix. The scanner returns only the first match, so a Stripe key may be reported as an OpenAI key or vice versa.
- **Impact:** Incorrect pattern attribution in logs. No functional impact on blocking behavior (the key is still blocked).
- **Suggested fix:** Order patterns by specificity (Stripe `sk_live_`/`sk_test_` before generic `sk-`) or return all matching patterns.

### GAP-020: Secret scanning has no custom pattern support

- **System:** Secret Scanning
- **SME Doc:** `SECRET_SCANNING_SME.md`
- **Files:** `crates/agent/src/secret_scan.rs`
- **Description:** All 15 regex patterns are compiled at startup via `OnceLock` and cannot be modified at runtime. There is no configuration file or API to add custom secret patterns. Users with proprietary key formats cannot extend the scanner.
- **Impact:** Organization-specific secrets (internal tokens, custom API keys) are not detected.
- **Suggested fix:** Support a `secret_patterns.yaml` config file or a settings.json array for user-defined patterns.

### GAP-021: Config silently uses defaults for missing YAML keys

- **System:** Configuration System
- **SME Doc:** `CONFIG_SYSTEM_SME.md`
- **Files:** `crates/config/src/lib.rs`
- **Description:** All config structs use `#[serde(default)]` on every field. If a required configuration key is missing from `nebo.yaml`, the system silently falls back to the struct default with no warning or validation.
- **Impact:** Misconfigured deployments may silently run with unintended defaults. Hard to diagnose configuration issues.
- **Suggested fix:** Add a `validate()` method on the root config struct that warns on critical missing values (e.g., missing JWT secret, missing NeboAI URLs).

### GAP-022: Linux platform tool backends missing

- **System:** Platform Tools
- **SME Doc:** `PLATFORM_TOOLS_SME.md`
- **Files:** `crates/tools/src/organizer/linux.rs`
- **Description:** Several organizer sub-tools have incomplete Linux backends. The tool attempts to detect installed backends (khal, gcalcli, notmuch, etc.) but many code paths return "missing backend" errors when no compatible tool is found.
- **Impact:** Linux users have limited platform tool functionality compared to macOS.
- **Suggested fix:** Document minimum requirements. Consider a "setup assistant" that helps Linux users install required backends.

### GAP-023: Comm framework — only NeboAI plugin implemented

- **System:** Communication Plugin Framework
- **SME Doc:** `COMM_FRAMEWORK_SME.md`
- **Files:** `crates/comm/src/manager.rs`
- **Description:** The `PluginManager` supports a single active communication plugin. The architecture is designed for multiple plugins (Slack, Discord, etc.) but only the NeboAI plugin is implemented. The manager routes all operations through the current plugin.
- **Impact:** No direct Slack/Discord/Teams integration. All external messaging goes through the NeboAI gateway.
- **Suggested fix:** This is by design (NeboAI gateway handles external services). Document the architecture decision.

---

## Low

Design debt, minor UX gaps, and incomplete platform coverage.

### GAP-024: Auth password reset timeout hardcoded

- **System:** Auth System
- **SME Doc:** `AUTH_SYSTEM_SME.md`
- **Files:** `crates/auth/src/service.rs`
- **Description:** Password reset token expiry is hardcoded to 1 hour. Not configurable via `nebo.yaml` or environment variable.
- **Impact:** Minor — 1 hour is a reasonable default. Only matters if deployment requires a different policy.

### GAP-025: Auto-updater staging workflow unclear

- **System:** Build Tooling
- **SME Doc:** `BUILD_TOOLING_SME.md`
- **Files:** `crates/updater/src/lib.rs`
- **Description:** The `update_pending` field uses `Arc<Mutex<Option<PathBuf>>>` to stage downloaded binaries, but the completion/application workflow has unclear edge cases around concurrent checks and interrupted downloads.
- **Impact:** Minor — edge case in auto-update flow.

### GAP-026: Workflow builder activity intent validation incomplete

- **System:** Workflow Builder UI
- **SME Doc:** `WORKFLOW_BUILDER_UI_SME.md`
- **Files:** `app/src/lib/components/workflow/BuilderCanvas.svelte`
- **Description:** Every activity requires a non-empty `intent` field. The frontend validates this but the error presentation and save-blocking logic has incomplete edge cases.
- **Impact:** Minor — users can work around by always filling intent fields.

### GAP-027: I18N currency formatting uses hardcoded dollar sign

- **System:** Internationalization
- **SME Doc:** `I18N_SYSTEM_SME.md`
- **Files:** `app/src/lib/` (various components)
- **Description:** Currency values throughout the UI use a hardcoded `$` prefix instead of `Intl.NumberFormat` with locale-aware currency formatting.
- **Impact:** Users in non-USD locales see dollar signs for all monetary values.
- **Suggested fix:** Use `new Intl.NumberFormat(locale, { style: 'currency', currency })`.

### GAP-028: I18N no ICU plural rules

- **System:** Internationalization
- **SME Doc:** `I18N_SYSTEM_SME.md`
- **Files:** `app/src/lib/i18n/locales/`
- **Description:** Nebo uses manual singular/plural key splitting (`item` / `items`) instead of ICU MessageFormat plural rules. This breaks for languages with complex plural forms (Arabic: 6 forms, Polish: 3 forms, Russian: 3 forms).
- **Impact:** Grammatically incorrect pluralization in ~8 supported locales.
- **Suggested fix:** Migrate to ICU `{count, plural, one {# item} other {# items}}` syntax in translation files.

### GAP-029: I18N RTL languages have no dir="rtl" support

- **System:** Internationalization
- **SME Doc:** `I18N_SYSTEM_SME.md`
- **Files:** `app/src/app.html`, `app/src/routes/+layout.svelte`
- **Description:** Arabic and Hebrew translations exist in locale files, but the `<html>` element never gets `dir="rtl"` set. No CSS logical properties (e.g., `margin-inline-start`) are used. The entire UI renders left-to-right for RTL locales.
- **Impact:** Arabic and Hebrew locales are unusable — text direction, alignment, and layout are all wrong.
- **Suggested fix:** Set `dir` attribute on `<html>` based on locale. Migrate CSS to logical properties.

### GAP-030: Desktop AX tree capture empty on Windows/Linux

- **System:** Desktop Tools
- **SME Doc:** `DESKTOP_TOOLS_SME.md`
- **Files:** `crates/tools/src/desktop_snapshot.rs`
- **Description:** `capture_ax_elements()` returns an empty result on Windows and Linux. Accessibility tree capture is only implemented for macOS.
- **Impact:** The "see-then-click" automation pattern (snapshot → identify element → click) doesn't work on Windows/Linux.
- **Suggested fix:** Implement AT-SPI2 for Linux and UI Automation for Windows.

### GAP-031: Desktop menu access missing on Linux/Windows

- **System:** Desktop Tools
- **SME Doc:** `DESKTOP_TOOLS_SME.md`
- **Files:** `crates/tools/src/desktop_tool.rs`
- **Description:** The `menu` resource has no implementation for Linux or Windows. Menu listing and clicking only works on macOS via AppleScript.
- **Impact:** Agents cannot automate application menus on non-macOS platforms.

### GAP-032: MCP server add wizard missing validation

- **System:** Frontend Settings
- **SME Doc:** `FRONTEND_SETTINGS_SME.md`
- **Files:** `app/src/routes/settings/mcp/+page.svelte`
- **Description:** The 3-step MCP server add wizard has a `configureDisabled` check for required fields but the validation implementation is incomplete. Invalid configurations can be submitted.
- **Impact:** Users can save MCP server configurations that fail at connection time.
- **Suggested fix:** Add URL format validation, connection test on save, and clear error messages.

---

## Cross-Cutting Themes

### 1. Hardcoded Values (affects 6+ systems)

Multiple systems use hardcoded constants that should be configurable:
- Embedding models and dimensions (GAP-014)
- Secret scanner patterns (GAP-020)
- Notification pagination (GAP-016)
- Password reset timeout (GAP-024)
- Desktop maximize dimensions (GAP-011)
- Config silent defaults (GAP-021)

**Recommendation:** Audit all `const` values and `OnceLock` patterns. Move user-tunable values to `nebo.yaml` or `settings.json`.

### 2. Missing Scheduled Cleanup (affects 3 systems)

Several DB tables grow unbounded because cleanup functions exist but are never called:
- Notifications (GAP-015)
- Completed tasks (GAP-018)

**Recommendation:** Add a daily `maintenance_sweep()` job to the scheduler that calls all cleanup functions.

### 3. I18N V2 Migration Incomplete (affects 4 gaps)

The V2 frontend rewrite dropped i18n wiring. Four separate gaps (GAP-007, GAP-008, GAP-009, GAP-013) stem from this incomplete migration.

**Recommendation:** Treat i18n V2 wiring as a single project with a tracking issue.

### 4. Platform Parity (affects 4 gaps)

Desktop and platform tools have significant gaps on non-macOS platforms:
- AX tree capture (GAP-030)
- Menu access (GAP-031)
- Input simulation (GAP-010)
- Linux backends (GAP-022)

**Recommendation:** Prioritize Windows coverage (larger user base) over Linux. Document platform limitations in user-facing docs.
