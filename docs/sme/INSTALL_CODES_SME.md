# Install Codes, Collections & the Install Modal — SME

> **Last verified:** 2026-06-13 (after the dependency-cascade unification + modal merge)
> **Scope:** Marketplace install codes (`SKIL-`/`AGNT-`/`PLUG-`/`APPS-`/`COLL-`/`CONN-`/`WORK-`/`LOOP-`/`NEBO-`), the dependency cascade, collection installs, the WebSocket progress protocol, and the `InstallFlowModal` UI.
> **Key files:** `crates/server/src/codes.rs`, `crates/server/src/deps.rs`, `app/src/lib/components/install/InstallFlowModal.svelte`, `app/src/lib/marketplace/installCodes.ts`, `app/src/lib/websocket/listeners.ts`

---

## 1. Mental Model

An install code is a `PREFIX-XXXX-XXXX` token (Crockford Base32 groups). Redeeming one always flows through **one canonical backend pathway** — `codes::detect_code()` → `codes::handle_code()` → a per-family handler — no matter where the code was entered (chat, marketplace sidebar, store product page, a Loop channel).

There is **ONE dependency cascade** (`deps::resolve_cascade`) and it **always installs**. Every *explicit* install — a pasted code, a marketplace install, a collection — force-installs its declared dependencies: choosing to install an artifact IS consent to install the components it requires. A **collection** (`COLL-`) is not a special installer; it's a named list of item codes that `handle_collection_code` converts to `DepRef`s and feeds to the same `resolve_cascade`.

The `auto_install_deps` setting (formerly `autonomous_mode`, which was only ever read by this cascade) now gates **only** the implicit boot-time reconcile in `lib.rs` (filesystem agents discovered on startup), so Nebo doesn't auto-pull a pile of deps for every agent on every launch without consent. Default OFF.

Progress is **not** in the HTTP/chat response. The redeem call blocks until everything is installed; all live detail is broadcast over WebSocket as `code_*` / `dep_*` events, which the frontend re-dispatches as `nebo:*` window CustomEvents consumed by `InstallFlowModal.svelte`.

```
entry point ──► detect_code ──► handle_code ──► handle_<family>_code
                                    │                 │ (collection)
                       broadcast "code_processing"    ├─ api.redeem_code(code)        ← NeboLoop round-trip
                                                      ├─ api.get_collection(id)       ← NeboLoop round-trip
                                                      ├─ items → Vec<DepRef>
                                                      └─ deps::resolve_cascade        ← always installs
                                                            │ broadcast "dep_cascade_start" {total}
                                                            │ per item: "dep_started" → install_dep() → "dep_installed"/"dep_failed"
                                                            │           (recurse into child deps)
                                                            └ broadcast "dep_cascade_complete" {installed,pending:0,failed}
                                    broadcast "code_result" (final summary)
```

---

## 2. Entry Points (where a code can be submitted)

| # | Entry | Path | Notes |
|---|-------|------|-------|
| 1 | **Chat composer** (user pastes code) | `app/src/lib/chat/controller.svelte.ts` → `ws.send('chat', …)` | `dispatchInstallStart()` opens the modal *locally* first (§7) |
| 2 | **Marketplace "Install code" box** | `app/src/routes/marketplace/+layout.svelte` (`redeemCode()`) | Also `ws.send('chat', …)`. ⚠️ Wrapped in `if (ws.isConnected())` — silently no-ops when WS is down |
| 3 | **Backend chat WS intercept** | `crates/server/src/handlers/ws.rs` | Every chat prompt is `detect_code()`-checked before reaching the LLM; codes never become model prompts |
| 4 | **Store product install button** | `crates/server/src/handlers/store.rs` (`install_store_product`, `POST /store/products/{id}/install`) | Fetches the product's code, routes through `handle_code()` |
| 5 | **Direct HTTP** | `POST /api/v1/codes` → `submit_code()` (`codes.rs`) | Returns the final summary JSON synchronously |
| 6 | **Loop/channel messages** | `crates/server/src/channel_dispatch.rs` (`handle_code_text`) | Codes pasted in a channel install non-interactively (modal auto-dismisses; `interactive: false`) |

Frontend code recognition is centralized in `app/src/lib/marketplace/installCodes.ts` — `CODE_RE`, `matchInstallCode()`, `dispatchInstallStart()`. One regex / one type map shared by every entry point.

---

## 3. Code Families & Dispatch

Detection: `detect_code()` (`codes.rs`). Dispatch: `handle_code()`.

| Prefix | CodeType | Handler (`codes.rs`) | Installs |
|--------|----------|----------------------|----------|
| `NEBO-` | Nebo | `handle_nebo_code` | NeboAI account pairing |
| `SKIL-` | Skill | `handle_skill_code` | SKILL.md / sealed .napp → skill loader |
| `WORK-` | Work | `handle_work_code` | Workflow (DB + `~/.nebo/workflows/<slug>/`) |
| `AGNT-` | Agent | `handle_agent_code` | Agent (DB + `~/.nebo/agents/<slug>/`), auth wizard, auto-activation |
| `LOOP-` | Loop | `handle_loop_code` | Join a Loop |
| `PLUG-` | Plugin | `handle_plugin_code` | Per-platform binary via `fetch_and_install_plugin` |
| `APPS-` | App | `handle_app_code` → delegates to `handle_agent_code` + `reconcile_app_fields` |
| `COLL-` | Collection | `handle_collection_code` | Every item via `resolve_cascade` (§4) |
| `CONN-` | Connection | `handle_connection_code` | MCP server config → same parser as Settings paste-import |

`handle_code()` brackets every install with broadcasts: `code_processing` (start), `code_result` (end), `chat_complete`.

---

## 4. Collection Install (`COLL-`) End-to-End

`handle_collection_code()` — `crates/server/src/codes.rs`.

1. **Redeem**: `api.redeem_code(code)` → NeboLoop `POST /api/v1/codes/redeem`. Payment-gated collections return early with a `checkout_url`.
2. **Fetch items**: `api.get_collection(artifact_id)` → NeboLoop `GET /api/v1/collections/{id}` → `items` array.
   ⚠️ NeboLoop-side `InstallCollection` **skips `private` items for non-owners** — a customer bundle must use `unlisted`/`invite_only` visibility.
3. **Map items → `DepRef`s**. Each item carries its own install `code`, `type`, `name`, `slug`. Type map: `skill`→Skill, `agent`|`app`→Agent (apps ARE agents), `plugin`→Plugin, `workflow`→Workflow. Items with no code or unknown type are **logged and skipped, never silently dropped**. `app` items set `has_app` for step 5.
4. **Install all**: `resolve_cascade(state, deps, &mut visited)` — installs every item and recurses into transitive deps.
5. **App reconciliation**: `reconcile_app_fields()` persists `is_app` / `app_ui_path` / `app_binary_path` / `app_window_config` so apps show up and launch as apps.
6. **Summary + setup sweep**: message `Installed collection "<name>": N of M items installed[, K failed]`; `sweep_plugin_auth()` finds installed plugins still missing credentials → broadcast `dep_needs_setup`.

The HTTP/chat call **blocks for the entire cascade** — no per-item timeout; each NeboLoop request has a 15s client timeout.

---

## 5. The Dependency Cascade (`crates/server/src/deps.rs`)

**One public installer**, `resolve_cascade(state, deps, visited)`, which always installs. (The old gated/force split was collapsed on 2026-06-13 — see git `d5efc37a`.) It calls `announce_cascade_start()` → broadcast `dep_cascade_start { total }`, then `resolve_cascade_inner`.

`resolve_cascade_inner`, per dep:

1. **Dedup/cycle check**: visited set keyed `DepType:reference` — recursion-safe.
2. **Already installed?** `is_installed()` — per-type presence checks: skills by filesystem (SKILL.md / extracted dir / .napp / manifest `code` match), workflows by DB code/name, plugins by slug in `plugin_store`, agents by DB id/name. If present → broadcast `dep_installed`, count it.
3. **Built-in refs**: simple names (no `@`, not a code) are `Unresolvable` — never sent to NeboLoop (`is_marketplace_ref`).
4. **Install**: broadcast `dep_started` → `install_dep()` → on success broadcast `dep_installed`, extract the artifact's own deps, **recurse**; on failure broadcast `dep_failed { error }`, count it, **continue with remaining items** (no abort).
5. Finally broadcast `dep_cascade_complete { installed, pending, failed }`. `pending` is always 0 (there is no pending/approve state any more). Inner recursion emits its own `dep_cascade_complete` per level — the modal does not route on this event, so nesting is harmless.

**Boot-time reconcile (the only gated callers):** two `tokio::spawn` sites in `lib.rs` reconcile filesystem agents on startup. Each is wrapped in `if crate::deps::auto_install_deps_enabled(&state)` (reads the `auto_install_deps` setting, default OFF).

Per-type installers (all redeem through NeboLoop then persist + reload the relevant loader): `install_agent`, `install_skill`, `install_workflow`, `install_plugin` (binary via `codes::fetch_and_install_plugin`). Child-dep extraction: `extract_agent_deps_from_frontmatter`, `extract_skill_deps`, `extract_workflow_deps`.

---

## 6. WebSocket Progress Protocol

All broadcasts go through `state.hub.broadcast(event, payload)` — **global, unscoped**. The frontend forwards them 1:1 as `nebo:<event>` window CustomEvents in `app/src/lib/websocket/listeners.ts`.

| Event | Payload | Emitted | Modal handler |
|-------|---------|---------|---------------|
| `code_processing` | `{ session_id, code, code_type, status_message, interactive }` | `handle_code` start (also synthesized locally by `dispatchInstallStart()`) | open + arm 30s safety net (code mode) |
| `dep_cascade_start` | `{ total }` | `announce_cascade_start` | sets `depTotal` (determinate bar) |
| `dep_started` | `{ depType, reference, name, slug }` | `resolve_cascade_inner` | row → spinner |
| `dep_installed` | same | already-present + fresh-install | row → ✓ |
| `dep_failed` | `+ error` | install failure | row → ✗ + per-row **Install** retry (`approveDeps`) |
| `dep_cascade_complete` | `{ installed, pending:0, failed }` | end of each cascade level | forwarded; the modal does not route on it (it routes on `code_result`) |
| `dep_needs_setup` | `{ items: [{slug,label,description,authType}] }` | collection + agent installs | "Needs setup" section → Settings → Plugins |
| `plugin_installing` / `plugin_installed` | `{ plugin }` | plugin pipeline | row updates |
| `plugin_auth_url` / `plugin_auth_complete` / `plugin_auth_error` | — | agent/plugin auth flows | drive the `auth` step |
| `code_result` | `{ success, message, artifact_*, payment_required, checkout_url, tier, interactive }` | `handle_code` end | done / error / confirm-purchase / → `loadSetup` for agents |

`dep_pending` is **no longer emitted** — the cascade never pends. (`agent_auth_required` is also no longer load-bearing for the UI: the merged modal reads plugin-auth needs from `getAgent().pluginsNeedingAuth` in `loadSetup`.)

---

## 7. Frontend: `InstallFlowModal.svelte` (app/src/lib/components/install/)

**One** component for every install+setup path (it replaced the old `CodeInstallModal` + `AgentSetupModal` on 2026-06-13 — git `f7587082`). Three launch modes converge on one phase machine:

- **`code`** — WS-driven: opens on a pasted code (`nebo:code_processing`). Mounted in `routes/marketplace/+layout.svelte` and `routes/[agentId]/+layout.svelte`.
- **`product`** — API-driven: caller sets `appId` + `show`; the modal calls `installStoreProduct`. Used by marketplace `LargeCard` (Get) and `ProductDetail` (Install).
- **`configure`** — edit an installed agent (`existingAgentId`), no install. Used by `ProductDetail` (Configure) and the `[agentId]/+layout` needs-setup auto-open.

**Phase machine** (conditional steps shown only when applicable):
```
installing (progress + dep cascade)
 → [confirm → processing]            payment, code mode only (payment_required)
 → loadSetup(agentId)                the join point: getAgent (inputFields, pluginsNeedingAuth,
                                      needsSetup) + listAgentWorkflows
 → [inputs]   if inputFields present   (AgentInputForm)
 → [auth]     if pluginsNeedingAuth    (per-plugin OAuth queue; env-type → Settings)
 → [schedule] if active heartbeat wf   (interval picker)
 → finalize(): activateAgent(agentId) → done
```

- **Join point `loadSetup`** reconciles the WS path and the API path: code mode calls it on `code_result` success for an agent/app (`artifact_id`); non-agent codes go straight to `done`. Product mode calls it after `installStoreProduct`. Configure mode calls it immediately.
- **Backend force-cascade means deps always settle** via `dep_*` events — the modal is a pure progress renderer; there is no `installDeps`/approve-on-complete workaround.
- **Global "Skip setup"** (footer during inputs/auth/schedule): `activateAgent(agentId)` immediately with defaults → `done`; unfilled required inputs / missing plugin auth remain flagged (the `done` step links to `/settings/plugins` and `/[agentId]/settings/configure`; existing `needsSetup` / `pluginsNeedingAuth` plumbing surfaces them later).
- **Dependency list** renders whenever `deps.length > 0`, in any phase — per-row status icon, `depLabel()`, copyable code, type tag, and per-row **Install** retry for failures.
- **30s safety net** (code mode): if no `code_result` arrives, the modal soft-completes.

---

## 8. Where Failures Surface

- One failed item never aborts the cascade — it lands as a `dep_failed` row with a retry button; the final message carries `N of M installed, K failed`; `code_result` is still `success: true`.
- **The single retry path** is `approveDeps({ deps: [...] })` → `POST /api/v1/deps/approve`, which re-enters `resolve_cascade` for just that dep. (The old `POST /agents/{id}/install-deps` endpoint was removed on 2026-06-13 — it was a duplicate force path.)
- Items NeboLoop omits (private, non-owner) or the mapper skips (no code / unknown type) produce no row and no failure — only a backend `warn!` log; the user sees a smaller total.

---

## 9. Status of Past Gaps / Limitations

**Fixed 2026-06-13:**
- ~~Stuck-pending deps~~ — explicit installs now force-cascade; `dep_pending` is gone.
- ~~Two divergent modals with duplicated auth/dep rendering~~ — merged into `InstallFlowModal`.
- ~~Duplicate force endpoints~~ — `install_deps` removed; `approve_deps` is the sole retry.

**Still true (UX, not correctness):**
1. **The blind window.** Item rows appear only as `dep_started` arrives (after `redeem_code` + `get_collection` round-trips). `dep_cascade_start` carries only a count. Pre-announcing the mapped `DepRef` list (or emitting a row per item up front) would show the full named checklist immediately.
2. **Single-item collections** keep the spinner (the determinate bar needs `progressTotal > 1`); the dep row still renders.
3. **Static `statusMessage`** during install — activity is conveyed by the row list + counter, not a "currently installing X" headline.
4. **Silent no-op when WS is down** — the marketplace `redeemCode()` opens the modal but only sends `if (ws.isConnected())`.
5. **Broadcasts are global/unscoped** — `dep_*` events carry no install id; concurrent installs could interleave. Mitigated in the merged modal: code-mode handlers guard on `mode`, and dep/auth handlers guard on `show`.

---

## 10. File Reference

| Concern | File |
|---------|------|
| Code detection, dispatch, per-family handlers, plugin binary install, app reconcile, auth sweep, `POST /api/v1/codes` | `crates/server/src/codes.rs` |
| The one cascade, presence detection, per-type installers, dep extraction, `auto_install_deps_enabled`, `approve_deps` | `crates/server/src/deps.rs` |
| Boot-time reconcile (gated cascade callers) | `crates/server/src/lib.rs` |
| `auto_install_deps` setting (renamed from `autonomous_mode`) | `crates/db/migrations/0104_rename_autonomous_mode.sql`, `crates/db/src/models.rs`, `crates/db/src/queries/settings.rs`, `crates/server/src/handlers/agent.rs` |
| Dep approval / retry route (`/deps/approve`) | `crates/server/src/routes/mod.rs` |
| Chat-WS / channel code intercept | `crates/server/src/handlers/ws.rs`, `crates/server/src/channel_dispatch.rs` |
| Store product install | `crates/server/src/handlers/store.rs` |
| NeboLoop REST client | `crates/comm/src/api.rs` |
| **The unified install + setup modal** | `app/src/lib/components/install/InstallFlowModal.svelte` |
| Canonical code regex / type map / instant-open dispatcher | `app/src/lib/marketplace/installCodes.ts` |
| WS → window event forwarding | `app/src/lib/websocket/listeners.ts` |
| Mount points | `app/src/routes/marketplace/+layout.svelte`, `app/src/routes/[agentId]/+layout.svelte`, `app/src/lib/components/marketplace/LargeCard.svelte`, `app/src/lib/components/marketplace/ProductDetail.svelte` |
| Settings toggle | `app/src/routes/settings/permissions/+page.svelte` |
