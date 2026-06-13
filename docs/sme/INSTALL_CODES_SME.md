# Install Codes, Collections & the Install Modal — SME

> **Last verified:** 2026-06-12 (against working tree)
> **Scope:** Marketplace install codes (`SKIL-`/`AGNT-`/`PLUG-`/`APPS-`/`COLL-`/`CONN-`/`WORK-`/`LOOP-`/`NEBO-`), the dependency cascade, collection installs, the WebSocket progress protocol, and the `CodeInstallModal` UI.
> **Key files:** `crates/server/src/codes.rs`, `crates/server/src/deps.rs`, `app/src/lib/components/chat/CodeInstallModal.svelte`, `app/src/lib/marketplace/installCodes.ts`, `app/src/lib/websocket/listeners.ts`

---

## 1. Mental Model

An install code is a `PREFIX-XXXX-XXXX` token (Crockford Base32 groups). Redeeming one always flows through **one canonical backend pathway** — `codes::detect_code()` → `codes::handle_code()` → a per-family handler — no matter where the code was entered (chat, marketplace sidebar, store product page, a Loop channel).

A **collection** (`COLL-`) is *not* a special installer: it is a named list of item codes. `handle_collection_code` resolves the list, converts every item to a `DepRef`, and feeds them all to the **one canonical multi-artifact installer**, `deps::resolve_cascade_force()` — the same machinery that installs a single agent's transitive dependencies.

Progress is **not** in the HTTP/chat response. The redeem call blocks until everything is installed; all live detail is broadcast over WebSocket as `code_*` / `dep_*` events, which the frontend re-dispatches as `nebo:*` window CustomEvents consumed by `CodeInstallModal.svelte`.

```
entry point ──► detect_code ──► handle_code ──► handle_<family>_code
                                    │                 │ (collection)
                       broadcast "code_processing"    ├─ api.redeem_code(code)        ← NeboLoop round-trip
                                                      ├─ api.get_collection(id)       ← NeboLoop round-trip
                                                      ├─ items → Vec<DepRef>
                                                      └─ deps::resolve_cascade_force
                                                            │ broadcast "dep_cascade_start" {total}
                                                            │ per item: "dep_started" → install_dep() → "dep_installed"/"dep_failed"
                                                            │           (recurse into child deps)
                                                            └ broadcast "dep_cascade_complete" {installed,pending,failed}
                                    broadcast "code_result" (final summary)
```

---

## 2. Entry Points (where a code can be submitted)

| # | Entry | Path | Notes |
|---|-------|------|-------|
| 1 | **Chat composer** (user pastes code as a message) | `app/src/lib/chat/controller.svelte.ts:534` → `ws.send('chat', { prompt: code, ... })` | `dispatchInstallStart()` opens the modal *locally* first (see §7) |
| 2 | **Marketplace sidebar "Install code" box** | `app/src/routes/marketplace/+layout.svelte:164-182` (`redeemCode()`) | Also `ws.send('chat', { prompt: code, agent_id: 'assistant' })`. ⚠️ Wrapped in `if (ws.isConnected())` — silently does nothing when WS is down (modal still opens; see §9.5) |
| 3 | **Backend chat WS intercept** | `crates/server/src/handlers/ws.rs:1465-1466` | Every chat prompt is checked with `detect_code()` *before* reaching the LLM; codes never become model prompts |
| 4 | **Store product install button** | `crates/server/src/handlers/store.rs:281-314` (`install_store_product`, `POST /store/products/{id}/install`) | Fetches the product's code, then routes through `handle_code()` with synthetic session `store-install-{id}` |
| 5 | **Direct HTTP** | `POST /api/v1/codes` → `submit_code()` `crates/server/src/codes.rs:1235` | Returns the final summary JSON synchronously |
| 6 | **Loop/channel messages** | `crates/server/src/channel_dispatch.rs:43-44` (`handle_code_text`) | Codes pasted in a channel install non-interactively (modal auto-dismisses; `interactive: false`) |

Frontend code recognition is centralized in `app/src/lib/marketplace/installCodes.ts` — `CODE_RE` (line 12), `matchInstallCode()`, `dispatchInstallStart()`. **One regex / one type map shared by every entry point** — a stale copy once silently dropped whole code families (the file header documents this bug class).

---

## 3. Code Families & Dispatch

Backend detection: `detect_code()` `codes.rs:41-84` (format `PREFIX-XXXX-XXXX`, Crockford Base32 charset). Dispatch: `handle_code()` `codes.rs:102-138`.

| Prefix | CodeType | Handler (`codes.rs`) | Installs |
|--------|----------|----------------------|----------|
| `NEBO-` | Nebo | `handle_nebo_code` :258 | NeboAI account pairing |
| `SKIL-` | Skill | `handle_skill_code` :266 | SKILL.md / sealed .napp → skill loader |
| `WORK-` | Work | `handle_work_code` :379 | Workflow (DB + `~/.nebo/workflows/<slug>/`) |
| `AGNT-` | Agent | `handle_agent_code` :663 | Agent (DB + `~/.nebo/agents/<slug>/`), auth wizard, auto-activation |
| `LOOP-` | Loop | `handle_loop_code` :924 | Join a Loop |
| `PLUG-` | Plugin | `handle_plugin_code` :937 | Per-platform binary via `fetch_and_install_plugin` :1103 |
| `APPS-` | App | `handle_app_code` :1211 | Agent path + `reconcile_app_fields` :1188 |
| `COLL-` | Collection | `handle_collection_code` :472 | Every item via `resolve_cascade_force` (§4) |
| `CONN-` | Connection | `handle_connection_code` :601 | MCP server config → same parser as Settings paste-import |

`handle_code()` brackets every install with broadcasts: `code_processing` (start, :115-126), `code_result` (end, :143-160), `chat_complete` (:179-182).

---

## 4. Collection Install (`COLL-`) End-to-End

`handle_collection_code()` — `crates/server/src/codes.rs:472-593`.

1. **Redeem** (:480-483): `api.redeem_code(code)` → NeboLoop `POST /api/v1/codes/redeem` (`crates/comm/src/api.rs:397-405`). Records the install for this bot, resolves the collection artifact.
2. **Payment gate** (:488-497): `status == "payment_required"` → return early with `checkout_url`; the modal runs the purchase flow (§7) and the code is re-processed after payment.
3. **Fetch items** (:500-508): `api.get_collection(artifact_id)` → NeboLoop `GET /api/v1/collections/{id}` → `items` array.
   ⚠️ NeboLoop-side `InstallCollection` **skips `private` items for non-owners** — a customer bundle must use `unlisted`/`invite_only` visibility. The desktop never sees the skipped items.
4. **Map items → `DepRef`s** (:510-555). Each item carries its own install `code`, `type`, `name`, `slug`. Type map: `skill`→Skill, `agent`|`app`→Agent (apps ARE agents), `plugin`→Plugin, `workflow`→Workflow. Items with no code or unknown type are **logged and skipped, never silently dropped**. `app` items set `has_app` for step 6.
5. **Force-install all** (:559-560): `resolve_cascade_force(state, deps, &mut visited)`. Force because pasting a collection code is an explicit request for the whole bundle — `autonomous_mode` only gates *implicit* dependency auto-install.
6. **App reconciliation** (:564-566): `reconcile_app_fields()` :1188-1209 reloads the agent loader and persists `is_app` / `app_ui_path` / `app_binary_path` / `app_window_config` to DB so apps show up and launch as apps.
7. **Summary + setup sweep** (:568-592): message `Installed collection "<name>": N of M items installed[, K failed]`; `sweep_plugin_auth()` :1301-1328 finds installed plugins still missing credentials → broadcast `dep_needs_setup`.

The HTTP/chat call **blocks for the entire cascade** — there is no per-item timeout; each NeboLoop request has a 15s client timeout (`crates/comm/src/api.rs:34-37`).

---

## 5. The Dependency Cascade (`crates/server/src/deps.rs`)

Two public entry points, one inner loop:

- `resolve_cascade()` :90 — normal path (agent/skill frontmatter deps); respects `autonomous_mode`, otherwise marks items `PendingApproval` (user approves via `POST /api/v1/deps/approve` → frontend `approveDeps()`).
- `resolve_cascade_force()` :109 — explicit-approval path (collections, user-approved pending deps); installs everything.

Both call `announce_cascade_start()` :101-106 → broadcast `dep_cascade_start { total }` so the modal can render a determinate bar.

`resolve_cascade_inner()` :118-266, per dep:

1. **Dedup/cycle check** (:132-136): visited set keyed `DepType:reference` — recursion-safe.
2. **Already installed?** `is_installed()` :293 — per-type presence checks: skills by filesystem (SKILL.md / extracted dir / .napp / manifest `code` match, :329), workflows by DB code/name (:373), plugins by slug in `plugin_store` (:307), agents by DB id/name (:316). If present → broadcast `dep_installed` and count it (so retried rows settle correctly).
3. **Built-in refs**: simple names (no `@`, not a code) are `Unresolvable` — never sent to NeboLoop (:160-172).
4. **Install** (autonomous/forced, :174-230): broadcast `dep_started` → `install_dep()` → on success broadcast `dep_installed`, extract the artifact's *own* deps, **recurse**; on failure broadcast `dep_failed { error }`, count it, **continue with remaining items** (no abort).
5. Non-autonomous: broadcast `dep_pending`, mark `PendingApproval`.
6. Finally broadcast `dep_cascade_complete { installed, pending, failed }` (:250-257). Note: inner recursion emits its own `dep_cascade_complete` per level — the modal ignores this event, so nesting is harmless today.

Per-type installers (all redeem through NeboLoop then persist + reload the relevant loader):

| Type | Installer | Persist | Child-dep extraction |
|------|-----------|---------|----------------------|
| Agent | `install_agent` :462 | `tools::persist_agent_from_api` + `agent_loader.load_all()` | `extract_agent_deps_from_frontmatter` :621 |
| Skill | `install_skill` :494 | `tools::persist_skill_from_api` + `skill_loader.load_all()` | `extract_skill_deps` :748 |
| Workflow | `install_workflow` :556 | `persist_workflow_artifact` (`codes.rs:1364`) | `extract_workflow_deps` :719 |
| Plugin | `install_plugin` :576 | `codes::fetch_and_install_plugin` (`codes.rs:1103`): platform binary → `download_napp` → `plugin_store.install_from_napp` → DB upsert → re-register tool + hooks | plugin manifest deps :605 |

---

## 6. WebSocket Progress Protocol

All broadcasts go through `state.hub.broadcast(event, payload)` — **global, unscoped** (every connected client gets them; see §9.6). The frontend forwards them 1:1 as `nebo:<event>` window CustomEvents in `app/src/lib/websocket/listeners.ts:272-297`.

| Event | Payload | Emitted | Modal handler |
|-------|---------|---------|---------------|
| `code_processing` | `{ session_id, code, code_type, status_message, interactive }` | `handle_code` start (`codes.rs:115`) — and synthesized locally by `dispatchInstallStart()` | `handleCodeProcessing` — `reset()`, open, arm 30s safety net |
| `dep_cascade_start` | `{ total }` | `announce_cascade_start` (`deps.rs:101`) | `handleDepCascadeStart` — sets `depTotal` (only while `phase === 'installing'`) |
| `dep_started` | `{ depType, reference, name, slug }` | `deps.rs:177` | row → spinner |
| `dep_installed` | same | `deps.rs:142` (already present) and `:188` (fresh install) | row → ✓ |
| `dep_failed` | `+ error` | `deps.rs:213` | row → ✗ + per-row **Install** retry button (`retryDep` → `approveDeps`) |
| `dep_pending` | same | `deps.rs:232` (non-autonomous only) | row added as pending |
| `dep_cascade_complete` | `{ installed, pending, failed }` | `deps.rs:250` | forwarded but **not consumed by the modal** (AgentSetupModal uses it) |
| `dep_needs_setup` | `{ items: [{slug,label,description,authType}] }` | collection :580, agent installs | "Needs setup" section → Settings → Plugins |
| `plugin_installing` / `plugin_installed` | `{ plugin }` | plugin pipeline | row updates (plugin slug as reference) |
| `agent_auth_required`, `plugin_auth_url`, `plugin_auth_complete`, `plugin_auth_error` | — | agent/plugin auth flows | `phase = 'auth'` queue (multi-plugin OAuth, `env`-type routes to Settings) |
| `code_result` | `{ success, message, artifact_*, payment_required, checkout_url, needsAuth, tier, interactive }` | `handle_code` end (`codes.rs:143`) | done / error / confirm-purchase / agent-setup handoff |

---

## 7. Frontend: `CodeInstallModal.svelte` (app/src/lib/components/chat/)

Mounted in `routes/marketplace/+layout.svelte:364` and `routes/[agentId]/+layout.svelte:1632` — driven entirely by the window events above (no props besides `show`/`onclose`/`onAgentSetup`).

**Phases** (line 14): `installing → confirm → processing → checkout → auth → done | error`.

- **Instant open:** `dispatchInstallStart()` (`installCodes.ts:51`) synthesizes a local `code_processing` event the moment the user submits, so the modal opens before the WS round-trip. The backend's real `code_processing` arrives moments later and re-runs `reset()` (safe: cascade events come after it).
- **Progress rendering** (lines 569-591): `progressTotal = max(depTotal, deps.length)`. If `> 1` → determinate `<progress>` bar with `settledCount/progressTotal`; **otherwise an honest spinner + the code** (this is the state in the "Installing collection…" screenshot — see §9.1).
- **Dependency list** (lines 729-770): renders whenever `deps.length > 0`, in *any* phase — per-row status icon (pending ring / spinner / ✓ / ✗), display name via `depLabel()` (name → last segment of `@org/type/name` → raw ref), copyable code, type tag, and per-row retry for failures.
- **Cancel/close** (line 163): clears timers, closes the modal. **It does not abort the backend install** — the cascade runs to completion regardless.
- **`interactive` flag:** desktop-initiated installs stay open until dismissed; channel/loop-triggered installs auto-close ~1.5s after done.
- **30s safety net** (lines 186-195): if no `code_result` within 30s of `code_processing`, the modal *pretends* completion ("`<Type>` installed — finalizing dependencies...") and fires the sidebar refresh. See §9.2 for why this misfires on big collections.
- **Purchase flow:** `payment_required` → fetch payment methods → `confirm` → `createMarketplaceSubscription()` → system-browser checkout → wait (5 min timer) for the post-payment `code_result`.
- **Post-install refresh:** `notifySidebarRefresh()` dispatches `nebo:agent_installed`; consumed by `Sidebar.svelte:37` (`loadAgents()`) and `[agentId]/+layout.svelte:227` (`loadAgentRoster()`). Skills/plugins/installed pages are lazy-loaded on navigation — no live refresh.

Related component: `app/src/lib/components/agent-setup/AgentSetupModal.svelte` — the agent-install wizard listens to the same `dep_*` events (incl. `dep_cascade_complete`) with its own `DepRow` state; it is the handoff target when an agent install `needsAuth`.

---

## 8. Where Failures Surface

- One failed item never aborts the cascade — it lands as a `dep_failed` row with a retry button; the final message carries `N of M items installed, K failed`, and the overall `code_result` is still `success: true` (HTTP 200).
- Per-row retry calls `approveDeps({ deps: [...] })` (`POST /api/v1/deps/approve`) which re-enters `resolve_cascade_force` for just that dep; the same `dep_*` events settle the row.
- Items NeboLoop omits (private items for non-owners) or that the mapper skips (no code / unknown type) produce **no row and no failure** — only a backend `warn!` log. The user just sees a smaller total.

---

## 9. Known Gaps / UX Limitations (verified 2026-06-12)

These explain why a collection install can sit on a bare spinner with no detail:

1. **The blind window.** Nothing item-level exists until `dep_cascade_start`, which fires only *after* `redeem_code` + `get_collection` (two sequential NeboLoop round-trips, each with a 15s client timeout) plus item mapping. Until then the modal can only show the spinner + code. The backend knows the item list at `codes.rs:556` but never tells the UI what the items *are* — `dep_cascade_start` carries only a count, and rows appear one-by-one as `dep_started` arrives. Pre-announcing all items (e.g. broadcasting the mapped `DepRef` list, or emitting `dep_pending` for every item up front) would let the modal show the full named checklist immediately.
2. **30s safety-net false "done".** `installTimeout` is armed once in `handleCodeProcessing` and only cleared by `code_result`. `dep_*` activity does **not** reset it — so any cascade taking >30s (easy for a multi-plugin collection with binary downloads) flips the modal to `done` ("installed — finalizing dependencies...") while installs are still running. The deps list keeps updating beneath the premature checkmark.
3. **Single-item collections never show the bar.** The determinate bar requires `progressTotal > 1`; a one-item collection keeps the spinner (the dep row does still render).
4. **`statusMessage` is static.** It stays "Installing collection..." for the whole run; only the row list and counter convey activity. There is no "currently installing X" headline.
5. **Silent drop when WS is down.** Marketplace `redeemCode()` opens the modal unconditionally but only sends the code `if (ws.isConnected())` — disconnected WS means the spinner spins forever (until the 30s fake-done).
6. **Broadcasts are global and unscoped.** `dep_*` events carry no session/install id; two concurrent installs (or another device on the same backend) would interleave rows in one modal.
7. **Cancel is cosmetic.** Closing the modal does not stop the synchronous backend cascade.
8. **Per-level `dep_cascade_complete`.** Recursive child cascades each emit their own complete event; harmless only because the modal ignores the event entirely.

---

## 10. File Reference

| Concern | File |
|---------|------|
| Code detection, dispatch, per-family handlers, plugin binary install, app reconcile, auth sweep, `POST /api/v1/codes` | `crates/server/src/codes.rs` |
| Dependency cascade, presence detection, per-type installers, dep extraction | `crates/server/src/deps.rs` |
| Dep approval endpoint (`/api/v1/deps/approve`) route registration | `crates/server/src/routes/mod.rs:85-92` |
| Chat-WS code intercept | `crates/server/src/handlers/ws.rs:1465` |
| Channel-message code intercept | `crates/server/src/channel_dispatch.rs:43` |
| Store product install | `crates/server/src/handlers/store.rs:281` |
| NeboLoop REST client (`redeem_code`, `get_collection`, `install_skill`, `install_workflow`, `get_plugin`, `download_napp`) | `crates/comm/src/api.rs` |
| Install modal (all phases + progress UI) | `app/src/lib/components/chat/CodeInstallModal.svelte` |
| Canonical code regex / type map / instant-open dispatcher | `app/src/lib/marketplace/installCodes.ts` |
| WS → window event forwarding | `app/src/lib/websocket/listeners.ts:272-297` |
| Marketplace code input | `app/src/routes/marketplace/+layout.svelte:159-182, 219-241` |
| Chat composer code intercept | `app/src/lib/chat/controller.svelte.ts:534` |
| Agent setup wizard (same `dep_*` events) | `app/src/lib/components/agent-setup/AgentSetupModal.svelte` |
