All findings are provided with confirmed verdicts. I'll produce the ranked report directly.

# CODE_AUDITOR Rule 8 — Confirmed Competing-Pathway Violations

## HIGH severity

### 1. Frontend store endpoints: raw `webapi` vs generated client
**Capability:** Calling the marketplace store HTTP endpoints (`/api/v1/store/*`, apply-update) from the frontend.
- **Generated canonical:** `app/src/lib/api/nebo.ts:1369-1517` — `listStoreProducts`, `listStoreFeatured`, `listStoreCategories`, `getStoreProduct`, `getStoreProductReviews`, `getStoreProductMedia`, `getStoreProductFeedback`, `submitStoreProductReview`, `submitStoreProductFeedback`, `applyUpdate` (nebo.ts:439).
- **Hand-rolled raw `webapi.get/post`:** `app/src/routes/marketplace/+page.svelte:42,63,64`; `+layout.svelte:31,72`; `search/+page.svelte:27`; `installed/+page.svelte:34-36`; `categories/+page.svelte:22`; `categories/[slug]/+page.svelte:22,40`; `ProductDetail.svelte:145-148,217,218,227,230`; `LargeCard.svelte:29`; `FeedbackSection.svelte:76,80`.

**Why one concern:** Every hand-rolled call hits an endpoint that already has a typed generated method. The project rule (feedback-generated-api-client-only) mandates the frontend reaches the backend ONLY via `$lib/api/nebo`. A parallel raw-fetch path to identical endpoints is the textbook competing pathway. `ProductDetail` is self-contradicting: it uses the generated client for install/uninstall but raw `webapi` for fetch/reviews/media/feedback on the same `/store/products/{id}` resource.

**Canonical path:** Keep the generated `nebo.ts` functions. Replace every `webapi.get/post('/api/v1/store/...')` and the apply-update POST with the matching generated function; regenerate via `make gen` if a param is missing. Drop the `<any>` casts. After migration, `/api/v1/store` string literals should live only in `nebo.ts`. (Note: `raw-fetch-vs-generated-store-client` and `store-products-raw-webapi-vs-generated` are the same finding at differing granularity — consolidate into this one.)

### 2. Plugin setup wizard: raw `fetch` vs generated `pluginSetupRun`
**Capability:** Running a plugin setup wizard step (`POST /api/v1/plugins/{slug}/setup`).
- **Generated canonical:** `app/src/lib/api/nebo.ts:1208-1209` — `pluginSetupRun(slug, req)` with typed `PluginSetupRunResponse` (zero importers — the dead canonical).
- **Raw window fetch:** `app/src/lib/components/SetupWizard.svelte:128` — bare `fetch('/api/v1/plugins/${slug}/setup', {method:'POST', ...})`, bypassing even the `webapi` layer (no auth header / base-url handling).

**Why one concern:** Identical endpoint and operation. Most severe form of the violation because it also bypasses the shared `webapi` auth/base-url plumbing.

**Canonical path:** Replace the raw fetch with `pluginSetupRun(slug, { stepIndex, values })`; drop the manual `JSON.stringify`/`res.json()`.

### 3. Two uninstall pathways for marketplace artifacts
**Capability:** Uninstalling a marketplace artifact (agent / skill / plugin) from a marketplace surface.
- **Plugin-only endpoint (wrong):** `app/src/routes/marketplace/installed/+page.svelte:64` — `confirmUninstall` calls `removePlugin(item.id)` → `DELETE /api/v1/plugins/{slug}` (`handlers/plugins.rs:639`, plugin teardown only). The installed page lists only agents/workflows/skills (never plugins), and `item.id` is the NeboAI product id, not a plugin slug — so the call no-ops: the worker keeps running, triggers stay registered, DB+fs persist, no WS broadcast.
- **Canonical store endpoint:** `app/src/lib/components/marketplace/ProductDetail.svelte:205` — `uninstallStoreProduct(itemId)` → `DELETE /api/v1/store/products/{id}/install` (`store.rs:320 uninstall_store_product`), which branches on artifact_type and fully tears down agents/skills, delegating plugins to `remove_plugin_by_slug`.

**Why one concern:** Both buttons perform the same user-facing uninstall but through two different backends. Competing pathway AND a correctness bug for agent/skill uninstalls.

**Canonical path:** Make `installed/+page.svelte` call `uninstallStoreProduct(item.id)` so all types route through `store.rs:uninstall_store_product`. Reserve `removePlugin` for the plugin settings surface (`settings/plugins/+page.svelte:224`, which is correct).

### 4. Skill loader reconcile: `reload_from_disk` vs `load_all`
**Capability:** Reconcile the in-memory skill loader after a mutating skill install so the new skill appears immediately.
- **Canonical:** `reload_from_disk()` — `handlers/skills.rs:212` (create), `:261` (update), `:292` (delete); `store.rs:441` (uninstall). Deletes the stale `.skill-manifest.json` then forces a cold scan.
- **Stale-prone variant:** `codes.rs:354` (`install_skill`) and `deps.rs:601` (dep cascade) call `load_all()` directly after writing files. `load_all()`'s warm path (`loader.rs:113`, `try_warm_load` `:146`) trusts the cached manifest and never diffs the filesystem, so the just-written skill is absent.

**Why one concern:** All four sites refresh the loader after a filesystem mutation. The loader's own doc comment (`loader.rs:125-136`) states "Mutating paths must call this [reload_from_disk], not load_all()." The install paths — which most need fresh state — use the wrong variant.

**Canonical path:** Change `codes.rs:354` and `deps.rs:601` to `reload_from_disk().await`. `load_all()` then remains only for startup warm-load (`lib.rs:786`).

### 5. Dual chat-title generator
**Capability:** Auto-generate a chat/session title from conversation content after the first exchange.
- **Runner-side:** `crates/agent/src/runner.rs:885-926` — background task calling `summarizer::generate_session_title`, gated on default-title-string match, fires once on the first turn. Active for all paths not setting `skip_title_gen`.
- **Dispatch-side:** `crates/server/src/chat_dispatch.rs:1872-1976` (`generate_chat_title_if_needed`, invoked at `:1179`) — builds its own `ai::ChatRequest`, streams from `provider.stream()`, gated on `title_custom` + `user_turns == 1 || == 3` (refines twice), distinct prompt. Used only by `run_chat`, which sets `skip_title_gen: true` (`chat_dispatch.rs:403`) to suppress the runner generator.

**Why one concern:** Both name a chat from its messages via a cheap LLM call, diverging in trigger logic and call construction. The `skip_title_gen` flag papering over the race is itself the tell of two competing pathways.

**Canonical path:** Keep dispatch-side `generate_chat_title_if_needed` (richer count-1/count-3 logic), move it into `summarizer.rs` so every run path can call it, delete the `runner.rs:885-926` block and the `skip_title_gen` field, and have the runner/post-run hook invoke the one generator unconditionally.

## MEDIUM severity

### 6. Plugin install/update: `fetch_and_install_plugin` vs `apply_plugin_update_pub`
**Capability:** Download a plugin's `.napp` and install it (detail → resolve binary → `download_napp` → `install_from_napp` → DB upsert).
- **Canonical (designated):** `crates/server/src/codes.rs:1098 fetch_and_install_plugin` — doc comment (`:1094-1097`) states it was extracted "so binary resolution and DB registration can't drift." Does `remove(slug)` + skill-watcher pause/load_all/resume + tool/hook re-register + real sha256/sig.
- **Drifted copy:** `crates/server/src/artifact_updates.rs:305 apply_plugin_update_pub` — re-implements the same sequence, omitting `plugin_store.remove()`, the skill_loader cycle, and tool/hook re-register; passes empty `binary_path`/`hash` to upsert and hand-rolls its own `current_platform()`.

**Why one concern:** Update is not distinct from install — `install_from_napp` already does stage-verify-swap over an existing version. The canonical fn was created precisely to stop the drift now present.

**Canonical path:** Make `apply_plugin_update_pub` call `codes::fetch_and_install_plugin(state, api, slug, &manifest.name)`; delete the duplicated body.

### 7. Plugin auth-status check (4 copies → exit-code drift)
**Capability:** Run a plugin's `auth.commands.status` subcommand and decide if it's authenticated.
- **Canonical (richest):** `crates/napp/src/plugin.rs:2363 run_auth_status_check` + `interpret_auth_status_output` (`:2400`) — honors `authenticated`/`isAuthenticated`/`logged_in` booleans and `none` credential_source/auth_method/storage. Backs `check_auth_lazy`/`refresh_auth_cache`/`update_auth_status`, feeds `is_ready()`.
- **Exit-code-only copies:** `crates/server/src/handlers/plugins.rs:715 check_plugin_auth` and `:733 auth_status`; `crates/tools/src/plugin_tool.rs:1047 run_auth_status`. All return `output.status.success()`.

**Why one concern:** All four answer "is this plugin authenticated?" The three callers re-spawn the binary and apply a weaker exit-code rule, disagreeing with the store copy for exit-0 "not connected" reporters (gws). Active bug surface, encoded in the unit test at `plugin.rs:2443`.

**Canonical path:** Make the handlers and `plugin_tool.rs` delegate to `PluginStore::check_auth_lazy` (or factor `run_auth_status_check` + `interpret_auth_status_output` into one shared fn) so the decision is computed in exactly one place.

### 8. `run_chat` vs `run_chat_events` dispatch setup
**Capability:** Chat dispatch entry — resolve display name, register in RunRegistry, build RunRequest, run on a lane, consume the event stream.
- **Broadcast sink:** `crates/server/src/chat_dispatch.rs:225 run_chat` (web-UI/scheduled/heartbeat; sets `skip_title_gen:true`).
- **Returned-channel sink:** `crates/server/src/chat_dispatch.rs:1230 run_chat_events` (channel/app callers; forwards events to an mpsc receiver).

**Why one concern:** The ~80-line preamble (display-name resolution, identical `RegisterParams`, RunRequest assembly, lane-task wrapping `runner.run`) is duplicated near-verbatim; only the terminal stream consumer differs. The copies have already drifted (`skip_title_gen`, `channel_ctx`). `run_chat_events` does not delegate to `run_chat`.

**Canonical path:** Extract shared setup (display-name + RunRegistry register + RunRequest build + lane-task) into one helper yielding the event stream; layer the two output behaviors (broadcast vs returned Receiver) as thin consumers.

### 9. Skills-list UI: `listTools` vs `listExtensions`
**Capability:** Enumerate and display installed skills in the desktop UI.
- **Canonical:** `app/src/routes/settings/skills/+page.svelte:29` → `listExtensions()` → `GET /api/v1/extensions` → `handlers/skills.rs:list_extensions` reads `skill_loader.list_summaries()` (the real loaded skill set). Linked from `SettingsShell.svelte:48`.
- **Broken parallel page:** `app/src/routes/skills/+page.svelte:20` → `listTools()` → `GET /api/v1/integrations/tools` → `integrations.rs:1110 list_tools` returns the STRAP domain-tool REGISTRY, relabeled as "skills." `toggleSkill` (`:33`) posts a tool name to the skill-dir toggle endpoint, which never resolves. Linked from `Sidebar.svelte:103`.

**Why one concern:** Both render an "Installed Skills" list via two data sources; the `/skills` page enumerates the wrong source and is broken.

**Canonical path:** Delete `app/src/routes/skills/+page.svelte` and the `Sidebar.svelte:103` entry, OR repoint it at `listExtensions()`. Keep `/settings/skills` as the single skill-enumeration UI; if a top-level nav is wanted, redirect to `/settings/skills`.

### 10. Agent enable/disable: `toggle_agent` vs `activate`/`deactivate`
**Capability:** Enable/disable an agent over HTTP (persist `is_enabled` + start/stop worker).
- **Incomplete subset (dead in UI):** `crates/server/src/handlers/agents.rs:891 toggle_agent` (route `roles.rs:27-29`). Flips DB flag + start/stop worker only — no `agent_registry` update, no app sidecar lifecycle, no owner-loop register, no WS broadcast. Generated `toggleAgent` (`nebo.ts:354`) has zero frontend callers; only `mvp_readiness.rs:536-542` uses it.
- **Canonical:** `agents.rs:1809 activate_agent` + `:1918 deactivate_agent` (routes `roles.rs:38-45`). Full pathway: persist + worker + registry + sidecar lifecycle + owner-loop + WS broadcast. Frontend uses these (`[agentId]/+layout.svelte:361-363`).

**Why one concern:** Both HTTP endpoints turn an agent on/off. `toggle_agent` is a behaviorally-divergent subset leaving registry/roster/runtime inconsistent with the DB — the RegisterWithToken-vs-Register anti-pattern.

**Canonical path:** Delete `toggle_agent` (`agents.rs:891-910`), its route (`roles.rs:26-29`), generated `toggleAgent` (`nebo.ts:354`) + `ToggleAgentResponse`; switch the `mvp_readiness.rs` test to `/activate`+`/deactivate`. (`store.toggle_agent` stays — still used by the agent tool at `agent_tool.rs:306`.)

### 11. `agent-runs` hand-rolled vs generated `listAgentRuns`
**Capability:** Listing an agent's runs (`GET /api/v1/agents/{id}/runs`).
- **Generated canonical:** `app/src/lib/api/nebo.ts:319-320` — `listAgentRuns(id, limit, offset)`, typed `ListAgentRunsResponse`.
- **Hand-rolled:** `app/src/routes/[agentId]/+layout.svelte:337` — dynamically imports `webapi` and reconstructs `webapi.get('/api/v1/agents/${id}/runs', {limit, offset})` with the same response type. (The same file uses the generated client idiomatically at `:359`, proving this is inconsistent.)

**Canonical path:** Replace the dynamic `webapi` import + hand-rolled get with `listAgentRuns(id, 20, current)`.

### 12. `apply-update` hand-rolled vs generated `applyUpdate`
**Capability:** Applying a pending artifact update (`POST /api/v1/artifacts/{id}/apply-update`).
- **Generated canonical:** `app/src/lib/api/nebo.ts:439` — `applyUpdate(id, req)`.
- **Hand-rolled:** `app/src/lib/components/marketplace/ProductDetail.svelte:217` — `webapi.post('/api/v1/artifacts/${itemId}/apply-update', {})`.

**Canonical path:** Replace with `applyUpdate(itemId, {})`. (Subsumed by Finding 1's recommendation; track together.)

### 13. Duplicate store wrappers in `index.ts` (name collision)
**Capability:** Typed wrapper functions for store product list/detail/reviews.
- **Generated canonical:** `app/src/lib/api/nebo.ts:1439,1453,1495` — `listStoreProducts`, `getStoreProduct`, `getStoreProductReviews`, all already accepting query params.
- **Hand-written duplicates:** `app/src/lib/api/index.ts:129-139` — same names, comment "generated API lacks param support" (factually false). `index.ts:2` does `export * from './nebo'`, so the local defs shadow the generated re-exports — making the canonical functions unreachable through the `$lib/api` barrel.

**Why one concern:** Two definitions of one named export = ambiguous canonical source + export-name collision. (`dup-store-client-fns` and `index-ts-duplicate-store-wrappers` are the same finding.)

**Canonical path:** Delete the three hand-rolled defs in `index.ts:129-139` and rely on the generated `nebo.ts` versions. Keep only genuinely-not-generated helpers (TTS/transcribe, OAuth state-param variants, marketplace-subscription fns).

## LOW severity

### 14. Redeem-code type-named aliases
**Capability:** Redeem a marketplace install code against `POST /api/v1/codes/redeem`.
- **Canonical:** `crates/comm/src/api.rs:397 NeboAIApi::redeem_code`.
- **Pure aliases:** `api.rs:408 install_skill`, `:582 install_workflow`, `:614 install_agent` — each a literal `self.redeem_code(code).await`. The names are meaningless: callers cross labels (`install_skill` redeems plugin codes at `codes.rs:935` and agent codes at `deps.rs:527`).

**Why one concern:** Same endpoint, body, return type; the artifact-type label carries no behavior — the RegisterWithToken-vs-Register anti-pattern. (The standalone `redeem_code` at `api.rs:1280` hits a different pre-auth endpoint and is correctly excluded.)

**Canonical path:** Delete `install_skill`/`install_workflow`/`install_agent`; update call sites (`codes.rs:270,384,665,935`; `deps.rs:527,561,622,644`; `workflow_manager.rs:293`) to call `redeem_code` directly.

### 15. Plugin OAuth-URL extraction (verbatim duplicate)
**Capability:** Drive a plugin's auth-login subprocess, scrape the OAuth URL from output, broadcast `plugin_auth_url`.
- **Server handler:** `crates/server/src/handlers/plugins.rs:1086 open_auth_url` / `:1099 has_url_candidate` / `:1116 extract_url` (driven by `spawn_plugin_login` `:276`).
- **Verbatim copy:** `crates/tools/src/plugin_tool.rs:1284/1300/1315` (driven by `run_auth_login` `:1080`) — the comment at `:1281` literally reads "URL extraction (duplicated from handlers/plugins.rs)."

**Why one concern:** Byte-identical `has_url_candidate`/`extract_url`; `open_auth_url` differs only in broadcast sink (ClientHub vs Broadcaster) — an argument, not a concern.

**Canonical path:** Extract the three fns + the stderr/stdout scan loop into one shared helper (e.g. in `napp::PluginRuntime`) parameterized by the broadcast callback; both `spawn_plugin_login` and `run_auth_login` call it. Delete the `plugin_tool.rs` copies.

### 16. Duplicate `strip_mcp_prefix`
**Capability:** Strip the MCP namespace prefix (`mcp__<server>__<tool>` → bare tool) for tool resolution.
- **Canonical:** `crates/tools/src/registry.rs:971-977` (used at `:406,475,561`; tested at `:1102`).
- **Byte-identical duplicate:** `crates/workflow/src/engine.rs:1346-1352` (used at `:884`; duplicate test at `:1426`).

**Why one concern:** Identical `splitn(3, "__")` logic for the same name-normalization purpose. Also runtime-redundant: the engine dispatches through `RegistryTool` (`workflow_manager.rs:1271`) which calls `Registry::execute`, stripping again. The workflow crate already depends on tools; only `registry.rs`'s private `fn` blocks reuse.

**Canonical path:** Delete the engine copy + its test; make `registry.rs::strip_mcp_prefix` `pub(crate)`/`pub` and call it at `engine.rs:884`.

### 17. Session-key parsing: inline `splitn` vs `keyparser`
**Capability:** Parse a structured session key (`agent:{id}:{channel}:{context_id}`).
- **Canonical:** `crates/agent/src/keyparser.rs:28 parse_session_key`.
- **Inline re-parse:** `crates/agent/src/runner.rs:1164-1171` — `splitn(4, ':')` to extract context_id, even though the same file already `use`s and calls `parse_session_key` at `:665` for the channel of the identical key.

**Why one concern:** Two parsers for one key grammar drift when the format changes. The module already uses the canonical parser for one field, then re-parses inline for another.

**Canonical path:** Extend `parse_session_key` to expose the context_id segment as a named field; replace the `runner.rs:1164-1171` inline block with a call reading that field.

---

**Counts:** HIGH: 5 · MEDIUM: 8 · LOW: 4 (17 confirmed findings; the two store-frontend pairs and the two `index.ts`-duplicate pairs are duplicate framings of the same underlying violations — collapsing those, ~13 distinct violations).