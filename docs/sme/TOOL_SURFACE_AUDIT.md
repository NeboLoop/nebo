# Tool Surface Audit — "One Way To Do Things"

**Status:** In progress (2026-06-06). All HIGH-severity findings + most MED fixed and verified (build green; web tool live-tested). Decisions applied: no legacy anywhere (hard-remove), vision+profile wired, all-at-once.
**Done:** web (V-WEB-1..6), os/sub-tools (V-OS-F1/F2/F7, V-SET-H1, V-OS-M1, V-MUSIC-M3), skill (V-SKILL-1/2/3), publisher/plugin/vm (V-PUB-1, V-PLUG-2, V-XTOOL-3), agent/registry (V-BOT-1/3/7/8, V-AGT-5), desktop (V-XTOOL-1, V-DESK-1), loop (V-LOOP-5/7), **vision+profile wired (V-BOT-2)**.
**Deferred (MED/LOW, not guaranteed-misfire):** V-BOT-4/6, V-AGT-2/3/4/9, V-LOOP-6 (share — needs reply-attach-flow care), V-MSG-1/2, V-DESK-4/5, V-SET-H2, V-OS-F4, V-PLUG-1/3, build_domain_schema structural migrations.
**Audited:** 2026-06-06. **Scope:** every model-facing STRAP tool's `schema()` (action/resource enums + params), `description()`, and dispatch/alias maps.

## Why this exists

The tool surface **is** the model's behavior spec. Like Go's single `for` loop eliminating the do-while/while/foreach decision, every redundant action or param in a tool schema is a fork the model can take wrong. For an LLM this is sharper than for a human: a second name for one operation (e.g. `get_page_text` aliasing `read_page`) doesn't just add confusion — it lets the model take a path that may silently fail, and gives it no way to know which is real.

**The rule:** exactly ONE way to do each operation, and the schema must make that one way obvious. Lenient *input* (accept old names → return a correction) is good; a second *documented* or *silently-working* pathway is a violation.

## What is already correct (do not "fix")

- **Only domain tools are registered.** `file`/`shell`/`grep`/`settings`/`music`/`keychain`/`spotlight` are composed inside `os` — not standalone. Bare/old names hit `tool_correction()` (`registry.rs` ≈957–1048) and return a redirect, never a second execution path. ✔
- **Delegation has one live pathway:** `agent(resource:"registry", action:"delegate")`. `PersonaTool` (`name()=="agents"`) is never registered standalone (`registry.rs` ≈721–730). ✔ (But see V-AGT-9: the dead standalone impl should be removed.)
- **Reference implementations:** `mcp` (one action), `app` (8 clean actions), `notebook` (`read`/`edit` + `edit_mode` param — the ideal "one action, behavior via param"). Use these as the model.
- **Install/marketplace** correctly funnels through the HIL code path (`codes.rs`); `plugin`/`skill`/`app` do not each re-implement install. ✔

## Canonical conventions (the decisions every fix must follow)

1. **snake_case** for all action names and params. No camelCase. No snake/camel duplicates.
2. **One verb per operation, project-wide:**
   - list/enumerate → **`list`** (not `catalog`, `active`, `menus`, `featured`, `popular`)
   - create/define → **`create`** (not `schedule`, `add`)
   - remove → **`delete`** (not `remove`, `cancel` — `cancel` is reserved for "stop a running thing")
   - stop a run → **`cancel`** (not `cancel_run`, `quit` — `quit` is reserved for OS app termination)
   - deliver outbound → **`send`** (not `notify`, `post`)
   - read page/content → **`read_page`** (not `get_page_text`, `snapshot`)
3. **`action` = the operation, always.** Never make `action` carry a setting/entity name with the operation inferred from param presence (the `settings` anti-pattern).
4. **Resource-scopes the action**; the same verb under different resources is fine (`channel/list` vs `group/list`) **if** resource selection is reliable and documented.
5. **Every param the dispatch reads MUST be in the schema.** An accepted-but-undocumented param is a hidden second pathway.
6. **Every action in the schema enum MUST be dispatched and work.** A documented action that errors ("not available") is a guaranteed wrong fork.
7. **Deprecated/removed names** return a correction pointing to the canonical form — never execute, never silently alias.
8. **Input shapes are identical across sibling tools.** "click/type/scroll" must look the same in `web` and `desktop`.

## Severity

- **HIGH** — model is actively misled (advertised form that fails) or a real capability is undiscoverable, or a silent wrong-path.
- **MED** — two plausible documented paths for one intent; cross-tool drift.
- **LOW** — undocumented alias that happens to work; cosmetic/naming.

> Line numbers are `≈` (they drift as fixes land). Anchor on the function name + snippet.

---

# TIER 0 — Correctness bugs (model is lied to → guaranteed misfire)

### V-WEB-1 — Duplicate `query` schema key drops the search guidance — HIGH ⬜
- **File:** `crates/tools/src/web_tool.rs`, `schema()` — `"query"` defined twice: ≈1345 (search guidance) and ≈1489 (`find` description).
- **Current:** JSON object construction keeps the LAST key, so the `find` description (`"For find: natural language description…"`) overwrites the search query guidance. **The anti-`site:`-spam guidance added 2026-06-06 is never seen by the model.**
- **Canonical fix:** one `query` property whose description covers both uses, OR a separate `find_query` — but prefer merging: keep the search guidance and append a "(for `find`: natural-language element description)" clause.
- **Defensive note:** this is a regression of tonight's search fix; fix first. Verify by dumping the emitted schema JSON and confirming the search guidance survives.

### V-WEB-2 — Dead devtools actions in the enum — HIGH ⬜
- **File:** `web_tool.rs`, `schema()` action enum ≈1327: `"console", "source", "storage", "dom", "cookies", "performance"`. `handle_devtools` (≈806–812) returns "not yet available" for all but `console`.
- **Canonical fix:** remove `source`/`storage`/`dom`/`cookies`/`performance` from the enum (keep `console`). If devtools is end-of-life, drop the whole `devtools` resource.
- **Defensive note:** confirm no skill/cron references these actions before removal; they'd just start getting an unknown-action error (acceptable).

### V-BOT-2 — `vision` & `profile` resources advertised but not dispatched — HIGH ⬜
- **File:** `bot_tool.rs`. `description()` advertises `agent(resource:"vision", action:"analyze", image:…)` (≈1620) and `agent(resource:"profile", …)` (≈1614); `is_concurrent_safe` references `profile` (≈1698). But neither is in the `resource` enum (≈1645), the auto-correct list (≈1720), nor the dispatch match (≈1735–1750) → falls through to "not available". `image`/`layer` params advertised, unread.
- **Canonical fix:** either wire the resources + params, or delete the description blocks + the `profile` arm in `infer_resource`/`is_concurrent_safe`.
- **Decision needed:** are vision/profile intended to exist? If yes → implement; if no → strip.

### V-BOT-8 — `agent_id` advertised, handler reads `task_id` — HIGH ⬜
- **File:** `bot_tool.rs`. `description()` ≈1593–1594 tells the model `task` status/cancel takes `agent_id`; handlers read `task_id` (≈683, 709); schema declares only `task_id` (≈1658).
- **Canonical fix:** description → `task_id` everywhere. (`task_id` is canonical.)

### V-AGT-5 — registry docs/errors use non-existent tool names — HIGH ⬜
- **File:** `agent_tool.rs`. `description()` examples say `agents(action:…)` (≈2452–2473); `missing_param` errors say `bot(resource:"registry", …)` (≈276, 536, 726, 1108, 1216, 2187, 2194); some say `agents(action:"list")` (≈367, 745, 1265).
- **Current:** the live form is `agent(resource:"registry", action:…)`. Three wrong names taught to the model.
- **Canonical fix:** sweep all docs/errors → `agent(resource:"registry", action:…)`. (See memory `tool-rename-bot-to-agent`.)

### V-PUB-1 — `publisher` tool, `publish(...)` examples — MED→HIGH ⬜
- **File:** `publisher_tool.rs`. `name()=="publisher"` (≈256) but every example/error calls `publish(...)` (≈27, 231, 266–269).
- **Canonical fix:** examples/errors → `publisher(action:"publish", …)`.

### V-OS-M1 — `spotlight`/`music` error strings reference deprecated `desktop(...)` — MED ⬜
- **Files:** `spotlight_tool.rs` ≈79 (`desktop(resource:"search", action:"files")` — both the tool name and action are wrong; live is `os(resource:"search", action:"search")`); glob redirects (≈98/130/151/188) omit `resource:"file"`. `music_tool.rs` ≈84–87 (`desktop(resource:"music"…)` → `os(resource:"music"…)`).
- **Canonical fix:** correct all to `os(resource:…, action:…)` with the right resource.

---

# TIER 1 — Undiscoverable real capabilities (the one way exists but is hidden)

### V-LOOP-5 — `channel/ensure` (the only way to create a channel) is not in the schema — HIGH ⬜
- **File:** `loop_tool.rs`. `channel ensure` fully implemented (≈201–221, `comm.ensure_channel`) and named in the dispatch error (≈325), but absent from the action enum (≈465) and its `name`/`description` params (≈202, 210) absent from schema properties (≈467–473).
- **Canonical fix:** add `ensure` to the action enum + `description()`, and document `name`/`description` params.
- **Defensive note:** verify the canonical create verb — per convention this should arguably be `create`, but `ensure` (idempotent get-or-create) is a real distinct semantic; keep `ensure` but document it.

### V-BOT-1 — `runs` resource dispatched but not in enum — HIGH ⬜
- **File:** `bot_tool.rs`. Dispatched (≈1742), in `infer_resource` (≈144), but absent from the resource enum (≈1645) and auto-correct list (≈1720). Error strings inconsistent (≈1731 omits it, ≈1752 includes it).
- **Canonical fix:** add `runs` to enum + auto-correct list; reconcile both error strings.

### V-OS-F1 — `regex` accepted for grep but removed from schema — HIGH ⬜
- **Files:** `os_tool.rs` schema documents only `pattern` (≈248, with a `// "regex" … removed from schema` comment ≈268); but `FileInput` still deserializes `regex` (`file_tool.rs:46`) and `handle_grep` falls back to it (`file_tool.rs:515–519`).
- **Current:** a model emitting `regex` (natural guess) silently works — schema and dispatch disagree.
- **Canonical fix:** remove the `regex` field from `FileInput` and the fallback in `handle_grep`. `pattern` is the one name.

### V-OS-F2 — shell `cwd`/`data`/`filter` accepted but undocumented — HIGH ⬜
- **Files:** `ShellInput` accepts `cwd` (`shell_tool.rs:27`), `data` (≈39, session write stdin), `filter` (≈35, process/session list). None in the `os` schema (`os_tool.rs` ≈293–308).
- **Current:** core capabilities (set working dir, write stdin, filter process list) are invisible; models guess `working_dir`/`stdin`/`input` and fork.
- **Canonical fix:** add `cwd`, `data`, `filter` to the schema with these exact names.

---

# TIER 2 — Cross-tool inconsistency (same concept, different shape — most insidious)

### V-XTOOL-1 — `desktop` input vs `web` browser input disagree on every shared param — HIGH ⬜
- **Files:** `desktop_tool.rs` (input resource) vs `web_tool.rs` (browser).

| Concept | desktop (current) | web (current) | **Canonical (adopt web's)** |
|---|---|---|---|
| coordinates | `x` + `y` (scalars) | `coordinate: [x,y]` | `coordinate: [x,y]` |
| element ref | `element_id` ("B3") | `ref` ("ref_1") + `selector` | `ref` |
| double/right click | actions `double_click`/`right_click` | `action:"click"` + `click_count` + `button` | `click` + `click_count` + `button` |
| scroll | `dx`/`dy` deltas | `direction` + `amount` | `direction` + `amount` |

- **Current danger:** desktop coords use `.as_i64().unwrap_or(0)`, so a web-shaped call (`coordinate:[x,y]`) **silently clicks (0,0)** instead of erroring (`desktop_tool.rs` ≈722–757).
- **Canonical fix:** align `desktop` input to web's shape (web is the newer, already-consolidated one). Accept the old `x`/`y`/`element_id` as undocumented-but-tolerated during transition; remove from docs.
- **Defensive note:** this is the highest-*value* fix but needs care — stage it: (1) add the canonical params + dispatch, (2) update docs, (3) keep legacy params working silently, (4) later remove legacy. Verify both tools' input via fixtures (`fixtures/tools/web-browser-interaction.yaml` + a desktop input fixture).

### V-XTOOL-2 — verb drift for one operation across tools — MED ⬜
- **"list":** `catalog`/`featured`/`popular` (skill), `active`/`list` (bot `runs`), `list`/`menus` (desktop menu), `list`/`status` (loop topic) → all should be **`list`** (+ a param for variants).
- **"deliver outbound":** `send` everywhere EXCEPT `message owner/notify` (uses action `notify`) → make it **`send`**.
- **"cancel a run":** `cancel` (task) vs `cancel_run` (runs) → **`cancel`**.
- **Canonical fix:** apply the verb glossary (conventions §2). Keep old verbs as accepted-but-undocumented corrections during transition.

### V-XTOOL-3 — `vm` uses camelCase actions — LOW ⬜
- **File:** `vm_tool.rs` ≈322, 374–385: `writeFile`/`readFile`/`copyOut` while everything else is snake_case.
- **Canonical fix:** `write_file`/`read_file`/`copy_out` (or reuse file vocab `read`/`write`).

---

# TIER 3 — Within-tool aliases & dual pathways

### V-WEB-3 — HTTP verb actions duplicate the `method` param — HIGH ⬜
- **File:** `web_tool.rs`. `action:"get|post|put|delete|head"` (enum) vs `action:"fetch", method:"POST"` (≈212–220). `fetch`/`get` are identical GETs (≈124, 215). PATCH only reachable via `method`, proving `method` is the real pathway.
- **Canonical fix:** keep `action:"fetch"` + `method` param; drop `get`/`post`/`put`/`delete`/`head` from the enum (accept as silent aliases → `fetch`+method during transition).

### V-WEB-4 — ~18 undocumented browser alias actions silently work — MED ⬜
- **File:** `web_tool.rs` `map_action_to_tool` / dispatch. In dispatch but NOT in schema enum: `double_click`, `triple_click`, `right_click`, `scroll_to`, `form_input`, `key`, `close`, `back`, `go_back`, `forward`, `go_forward`, `find_elements`, `console_messages`, `network_requests`, `resize`, `upload_file`, `snapshot`, `zoom`. Undocumented forwarded params: `x`, `y`, `scroll_direction`, `scroll_amount`, `tabId`, `tabIds`, `region`.
- **Canonical forms:** `click`+`click_count`/`button`; `scroll`(+`ref`); `fill`; `press`; `close_tab`; `history`+`direction`; `find`; `read_console_messages`; `read_network_requests`; `resize_window`; `file_upload`; `read_page`.
- **Canonical fix:** these are *aliases of canonical actions* — keep accepting them (lenient input) but ensure the canonical is the only documented one. The real issue is `back`/`go_back`/`forward`/`go_forward` (5 ways to navigate history vs `history`+`direction`) and `double_click`/`right_click` (Tier-2 alignment). Decide per-action: silent-alias-ok vs remove. Document the decision.

### V-WEB-5 — element targeting: 4 params — MED ⬜
- `ref`/`selector`/`coordinate`/`x`+`y` for click (`x`/`y` not even in schema, ≈1661/1663).
- **Canonical fix:** `ref` primary, `selector` fallback; `coordinate` only for raw clicks; remove bare `x`/`y` (fold into `coordinate`). Align with V-XTOOL-1.

### V-WEB-6 — `wait` duration: `ms` vs `duration` — MED ⬜
- Two params, two units (ms vs seconds), two documented maxes (≈1422 `ms`=10000, ≈1448 `duration`=30s), both forwarded (≈1673).
- **Canonical fix:** one param `ms` (integer milliseconds).

### V-SKILL-1 — `list` alias of `catalog` — HIGH ⬜
- **File:** `skill_tool.rs` ≈177 `"catalog" | "list" =>`. `list` accepted, not in enum (≈110).
- **Canonical fix:** per glossary, **`list`** should be canonical (not `catalog`) — rename `catalog`→`list` across schema/description/dispatch, OR keep `catalog` and drop `list`. **Decision:** align to the global `list` verb → rename `catalog`→`list`.

### V-SKILL-3 — `featured`/`popular` are fake marketplace browsing — MED ⬜
- **File:** `skill_tool.rs` ≈660 (`featured`), ≈684 (`popular`). Both re-sort/filter the **local** install list; no marketplace signal exists. `discover` (search) and `catalog`/`list` are the real ops.
- **Canonical fix:** remove `featured` + `popular` from the enum/dispatch. If marketplace ranking is wanted later, wire to the NeboAI API as a `sort` param on `list`.

### V-SKILL-2 — `body` accepted for `review` — LOW ⬜
- `skill_tool.rs` ≈942 `input["review"].or_else(|| input["body"])`. `body` undocumented.
- **Canonical fix:** drop the `body` fallback; `review` is canonical.

### V-LOOP-6 — `share` vs `send(path)` — two ways to send a file — HIGH ⬜
- **File:** `loop_tool.rs`. `send` with `path` uploads + attaches inline (≈132/149–157, 225/244–252). `share` (≈186, 320) defers attachment to reply-time (≈60–104). Description advertises `share` as the file-share way (≈442–443), competing with `send(path)`.
- **Canonical fix:** decide the one file-send semantic. Recommend: `send(path)` is the canonical "send a file now"; if the deferred reply-attach is a distinct need, rename `share` to convey "attach to my current reply" — otherwise remove it.

### V-LOOP-7 — `topic list` == `topic status` — MED ⬜
- `loop_tool.rs` ≈414–423 `"list" | "status" =>` identical; both return connection status (not a topic list).
- **Canonical fix:** keep `status` (accurate verb); drop `list` from topic.

### V-MSG-1/2 — `owner/notify` vs `notify/send`; verb `notify` vs `send` — MED ⬜
- **File:** `message_tool.rs`. `owner/notify` (≈130) and `notify/send` (≈172) both hit `notify_crate::send` (owner also writes companion chat). The deliver verb is `notify` under `owner` but `send` under `notify`.
- **Canonical fix:** use `send` as the deliver verb across resources; sharpen descriptions so `owner` (companion chat) vs `notify` (OS notification) is unambiguous. Lead `message` vs `loop` descriptions with the **recipient discriminator** (phone/owner → `message`; agent UUID/channel → `loop`).

### V-OS-S5 / V-SET-H1 — settings `mute`/`unmute` alias + inverted action model — MED ⬜
- **File:** `settings_tool.rs` ≈50, 89–90; on Windows both send the identical toggle key (≈835–839). Every other on/off setting uses one `toggle`+`value`.
- **Canonical fix:** `os(resource:"settings", action:"mute", value:true|false)` — drop `unmute`.
- **V-SET-H2 (MED):** the inner `SettingsTool` action enum (`get`/`set`/`status`/`toggle`/`trigger`, ≈52–56) is **dead on the `os` path** (the `os` wrapper makes `action`=setting-name and infers the op from `value`, `os_tool.rs` ≈575–613). Fix the inner schema/description (≈33–38) to match the live `os` shape, or pass `action` through unchanged. (Resolve the action-is-setting-name anti-pattern per convention §3.)

### V-AGT-2 — `automations` vs `agent_json` (create) — MED ⬜
- `agent_tool.rs` ≈543–561. Both define workflow bindings; schema admits `agent_json` desc says "use automations instead" (≈2598).
- **Canonical fix:** `automations` only on the model surface; keep `agent_json` parsing internal or hard-deprecate.

### V-BOT-4 — `research`/`submit_findings` worker-internal actions on the public surface — MED ⬜
- `bot_tool.rs` ≈1314–1315. `deep_research` is the canonical entry; `research`+`submit_findings` are the harness's internal worker protocol leaking into model-facing actions.
- **Canonical fix:** gate `research`/`submit_findings` out of the public action set (internal-only path).

### V-BOT-6 — `task delete` soft-skips (misnamed) — LOW ⬜
- `bot_tool.rs` ≈861–872: `delete` sets status `skipped`, doesn't delete. `cancel` (≈682) cancels a run.
- **Canonical fix:** rename to honest semantics (e.g. `skip`) or remove; don't expose both `cancel` and a fake `delete`.

### V-BOT-3 — `runs` `active`==`list`; `cancel_run` vs `cancel` — MED ⬜
- `bot_tool.rs` ≈1262 `"active" | "list"`. Cross-resource: `cancel_run` (runs) vs `cancel` (task).
- **Canonical fix:** `list` + `cancel` (drop `active`, rename `cancel_run`→`cancel`). Folds into V-XTOOL-2.

### V-BOT-7 — `task create` `description` vs `details` — LOW ⬜
- `bot_tool.rs` ≈772–774: `description` or undocumented `details`.
- **Canonical fix:** drop `details`; `description` canonical.

### V-AGT-9 — dead standalone `DynTool for PersonaTool` (`name=="agents"`) — LOW ⬜
- `agent_tool.rs` ≈2409–2411 + its own `schema()`/`description()`. Never registered standalone (folded into `agent` as `registry`), but the full second-tool surface exists as dead code and would re-introduce a dual pathway if ever registered.
- **Canonical fix:** delete the standalone `DynTool for PersonaTool` impl (keep the struct + `handle_action`).

### V-AGT-3/4 — `update` automation-mutation params; explicit-vs-inferred trigger — MED/LOW ⬜
- `agent_tool.rs`: `automations`(replace-all)/`add_automations`/`remove_automations`/`update_automation`/`toggle_automation` (≈973–1027) — 5 params mutate one set. And `trigger` is both an explicit enum and inferred from `schedule`/`interval`/`sources`/`plugin` presence (≈867–880).
- **Canonical fix:** make destructive replace its own explicit action; rely on trigger inference (drop explicit `trigger` except `manual`).

### V-PLUG-1/2 — `command` flags vs `args` map; `caption`/`title` — MED/LOW ⬜
- `plugin_tool.rs`: `command:"gmail +triage --max 5"` vs `args:{"max":"5"}` both feed the flag vector (≈670–685, 864–867) — justified escape hatch; tighten `args` doc to "only for values with shell metacharacters." `caption`/`title` upload alias (≈1320) — drop undocumented `title`.

### V-OS-F7 — glob action takes `pattern` OR `glob` — LOW ⬜
- `file_tool.rs` ≈424: glob expression via `pattern`, `glob`, or embedded in `path`. The `glob` field is documented as grep's file-filter (`os_tool.rs` ≈275).
- **Canonical fix:** glob action accepts only `pattern`; reserve `glob` field for grep's file filter.

### V-OS-F4 — `search`/`grep`/`glob` under-documented (when to use which) — MED ⬜
- Not redundant (OS-index search vs regex-in-files vs filename match) but the descriptions don't tell the model when to pick which (`os_tool.rs` ≈200).
- **Canonical fix:** documentation — each description states its one use case.

### V-DESK-1 — `capture`/`screenshot` alias — HIGH ⬜
- `desktop_tool.rs` ≈1425 `"screenshot" | "capture" =>`. `capture` undocumented; also self-referential under `resource:"capture"`.
- **Canonical fix:** `screenshot` only; drop `capture` alias.

### V-DESK-4/5 — `menu list`/`menus` and `dialog detect`/`list` near-aliases — MED ⬜
- `desktop_tool.rs`: `menu list` (top-level) vs `menus` (one level deep) ≈2218/2224 → one `list` + `depth` param. `dialog detect` vs `list` ≈2456/2457 → one `list` (empty list answers "any dialog?").

### V-MUSIC-M3 — `shuffle` set semantics undocumented — MED ⬜
- `music_tool.rs` ≈181–217: `shuffle` with `value` sets via `as_bool()`, but `value` is schema-typed `integer` (volume). No documented way to set shuffle.
- **Canonical fix:** give shuffle an explicit `enabled` boolean, or document `value` accepting bool for shuffle.

### V-DESK-structural — desktop `action` not enumerated in schema — MED ⬜
- `desktop_tool.rs` `schema()` ≈93–96: `action` is a free-form string; valid values only in prose. Root enabler of hidden aliases.
- **Canonical fix:** enumerate per-resource actions in the schema (as the other domain tools do via `build_domain_schema`).

### V-LOOP/MSG-structural — hand-rolled schemas drift from dispatch — MED ⬜
- `message_tool.rs` and `loop_tool.rs` do NOT use `build_domain_schema`/`validate_resource_action` (`domain.rs`); schema enums are maintained separately from dispatch match arms → the V-LOOP-5/V-LOOP-7 drift.
- **Canonical fix:** migrate both to `build_domain_schema` so schema is derived from one resource/action source of truth.

---

## Fix phases

- **Phase A (Tier 0):** V-WEB-1, V-WEB-2, V-BOT-2, V-BOT-8, V-AGT-5, V-PUB-1, V-OS-M1. Pure correctness, doc/enum edits, low risk. Start with **V-WEB-1** (regression of tonight's search fix).
- **Phase B (Tier 1):** V-LOOP-5, V-BOT-1, V-OS-F1, V-OS-F2. Surface hidden real capabilities.
- **Phase C (Tier 2):** V-XTOOL-1 (desktop↔web input — staged), V-XTOOL-2 (verb glossary), V-XTOOL-3.
- **Phase D (Tier 3):** alias/dual-pathway collapses + the structural `build_domain_schema` migrations (V-LOOP/MSG-structural, V-DESK-structural).

## Verification per fix
- Dump the emitted tool schema JSON and diff (especially V-WEB-1 — confirm search guidance survives).
- Run the relevant fixture live (`nebo-cli test run --fixture … --server localhost:27895`): `web-search-vs-fetch`, `web-browser-interaction`, `agents-delegate`, `agent-vs-agents`.
- For removed actions/params: confirm a legacy call returns a correction (not a silent wrong path or panic).
