# Memory Quality & Topical Taxonomy — Design

> **Status:** Approved design, not yet implemented (2026-06-12)
> **Owner context:** Follows the 2026-06-12 memory audit (1,980 dev / 75 prod memories: ~50% junk,
> `access_count = 0` on 99.8%, 710 orphaned `daily/*` entries) and the extraction of Claude's
> memory architecture from its binaries (`~/claude-source/docs/memory-prompts-extracted.md`).
> **Related:** `docs/sme/MEMORY_AND_PROMPT.md` (system reference — update §2/§3/§11 when this ships)

## Summary

Five changes, shipped as independently measurable rounds:

1. **Extraction prompt v2** — a durability bar with negative rules; `decisions` + `task_context`
   categories deleted; sensitive-info denylist added.
2. **Stage-0 deterministic filters** on every write path (also closes the explicit-store
   secret-scan/injection bypass).
3. **Taxonomy v2** — `daily/` deleted as a concept; new topical layer `project/<slug>` by default,
   **agent-declarable topics** via `agent.json` (`lead/`, `listing/`, `matter/`, …).
4. **Consolidation alignment** — durable-vs-dated semantics; consolidation is the *only* reaper
   (no TTL machinery). One-time sweep retires the legacy `daily/*` corpus.
5. **Deliberate-store path** — the main agent stores immediately on explicit "remember…" requests
   and on behavioral corrections; the sidecar remains the passive floor.

Explicitly **rejected**: a write-side LLM judge/gatekeeper (see Decisions), and any TTL-based expiry.

## Decisions & rejected alternatives

| Decision | Rationale |
|---|---|
| **No write-side LLM judge** (v1) | ~40% of audited junk is mechanically detectable (regex-killable); most of the rest was the extractor obeying a bad spec (`task_context` literally asked for "dates, budgets, quantities, locations"). The extractor is already an LLM — a self-grade bar in the same call is one fewer pathway than a second judging call. Claude ships zero write-side judging at far larger scale. **Revisit only if** the harness shows residual semantic-junk rate that the prompt rewrite + filters don't fix. |
| **Keep the sidecar extractor** | Nebo-specific: Janus routes to weak models that ignore prompt instructions (validated by the steering work), and the ICP never says "remember this" — capture must be passive and uniform across providers. Claude can rely on a deliberate strong-model writer; Nebo cannot. The sidecar's weakness (degraded transcript, no salience signal) is a spec problem, fixed by prompt v2. |
| **Kill `daily/`** | Date-as-namespace duplicates `created_at`; recall is topical, not temporal; since the identity-slice change dailies are never injected — a write-only graveyard (710 dead April entries). |
| **Topical layer, agent-declarable** | "Project" is generic exactly where agents are specialists. Topic names + descriptions *are* extraction instructions. Claude's types are fixed because it serves one domain; Nebo is a multi-domain agent platform — this is the right divergence. |
| **No TTL machinery** | Daily needed time-based expiry; topics need *semantic* expiry ("is this done/dated?") which consolidation already runs. Deleting daily deletes the need for TTL. |
| **Curation keeps its name** | It is **memory consolidation** (`memory_consolidation.rs`). Never "dream"/"auto-dream" — that's Claude's branding. |
| **Ephemera are not stored** | The session layer owns in-flight state (`active_task`, rolling summary, work tasks). Memory stopped duplicating it. The durable residue of a task is by definition a topic fact. |

**The spine:** session owns the ephemeral → the topical layer owns the ongoing → `tacit/` owns the
permanent → consolidation is the only reaper.

## Taxonomy v2

| Layer | Maps to Claude's | Lifecycle | Notes |
|---|---|---|---|
| `tacit/preferences` | `user` + `feedback` | permanent; reinforcement | invariant — powers identity-slice injection |
| `tacit/personality` | — (Nebo advantage) | decay + synthesis | invariant |
| `tacit/artifacts` | — | consolidation prunes | invariant |
| `<topic>/<key>` (default `project/`) | `project` | **retired by consolidation when done/dated** | agent-declarable, see below |
| `entity/<kind>/<name>` | — (Claude folds into prose) | consolidation prunes | kept: contacts/clients are first-class for the ICP; extraction bar raised |
| ~~`daily/<date>`~~ | — | **deleted as a concept** | legacy corpus swept once |

Claude's `reference` type (URLs/dashboards/tickets) gets no dedicated layer in v1 — such facts
land in the relevant topic.

## Agent-declared topics (`agent.json`)

```json
{
  "memory": {
    "context_isolated": true,
    "topics": [
      { "slug": "lead",    "description": "A prospective buyer or seller — stage, budget, timeline, next action" },
      { "slug": "listing", "description": "A property being marketed — address, price, status, showings" }
    ]
  }
}
```

```rust
// crates/napp/src/agent.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryTopic {
    pub slug: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryConfig {
    pub context_isolated: bool,
    #[serde(default)]
    pub topics: Vec<MemoryTopic>,
}
```

**Semantics**

- `topics` is **optional**. Declared topics replace only the generic `project/` category in that
  agent's extraction prompt; each `description` is injected verbatim as the category definition.
  Undeclared → default `project/`. Main Nebo companion uses `project/`. Existing agents: zero
  behavior change.
- Each topic is a namespace prefix inside the agent's existing memory scope
  (`{owner}:agent:{id}` chain) — e.g. `lead/john-and-amy-baker`. Scoping/inheritance unchanged.
- The invariant layers (`tacit/*`, `entity/`) are untouchable regardless of declared topics —
  identity injection, personality synthesis, and owner-scope inheritance never vary per agent.
- The memory tool's `layer` param additionally accepts the agent's declared slugs, validated
  against its config.
- Lifecycle is name-agnostic: consolidation's durable-vs-dated test runs per prefix. A closed
  lead retires exactly like a finished project. No per-topic lifecycle config.

**Validation** (agent.json load + marketplace scan):
slug matches `^[a-z0-9]+(-[a-z0-9]+)*$`; ≤ 8 topics; description ≤ 120 chars, non-empty;
reserved slugs rejected: `tacit`, `entity`, `daily`, `project`, `memory`, `style`, `artifact`.
The cap bounds extraction-prompt growth (~150 tokens for 8 topics).

## Extraction prompt v2

Replaces the current 6-category prompt in `crates/agent/src/memory.rs`. The `{TOPIC_CATEGORIES}`
block is assembled per scope from the agent's declared topics (default shown).

```text
Analyze the conversation and extract durable facts worth remembering in FUTURE conversations.

THE BAR — every fact must pass all three:
1. Will this still matter in a month?
2. Is it hard to re-derive on demand (not available from the user's files, calendar, tools, or this session)?
3. Is it about the user or their ongoing work — not about this conversation's mechanics?

Empty arrays are normal. Most conversations contain nothing durable.

Return a JSON object with these arrays:
1. "preferences" — preferences and corrections about how to work, stated or demonstrated.
   Include the why when the user gave one.
2. "styles" — communication/personality style observations (key: "style/trait-name")
3. "entities" — people, organizations, and places with significance beyond the current task
   (key: "kind/name", e.g. "person/sarah"). A name mentioned in passing is NOT an entity.
4. "topics" — ongoing work: goals, decisions, constraints, current status. Convert relative
   dates to absolute. Each fact names its topic:
   {TOPIC_CATEGORIES}
   - "project" — ongoing work, goals, or constraints the user would expect you to know next time
5. "artifacts" — important produced content worth referencing later (key: "artifact/description")

NEVER extract:
- times, dates, counts, quantities, IDs, or file paths standing alone
- session mechanics: which tools ran, message/input sizes, the current date, iteration details
- anything trivially re-derivable from the user's files, calendar, or connected tools
- secrets, credentials, API keys
- sensitive personal information — protected attributes (race, ethnicity, national origin,
  religion, age, sex, sexual orientation, gender identity, immigration status, disability,
  serious illness, union membership), government identifiers, financial account numbers,
  health information, home addresses — UNLESS the user explicitly asked you to remember it

Each fact:
- "key": unique, descriptive, path-like ("category/name")
- "value": 1-2 self-contained sentences (readable without this conversation)
- "topic": (topics array only) one of the topic slugs above
- "tags": searchable tags
- "explicit": true if the user directly stated it, false if inferred
```

Changes vs v1: `decisions` and `task_context` deleted (both wrote to `daily/`; task_context was
the worst junk producer); durability bar + NEVER list added (the bar is the in-call self-grade
that replaces the rejected judge); sensitive-info denylist adopted verbatim from Claude;
"empty is normal" added to stop extraction-for-extraction's-sake.

`format_for_storage` mapping v2: `preferences→tacit/preferences`, `styles→tacit/personality`,
`entities→entity/<kind>/<name>`, `topics→{topic}/{key}` (topic validated against the scope's
declared list, else `project/`), `artifacts→tacit/artifacts`.

## Stage-0 deterministic filters

Applied in `store_facts()` **and** the explicit memory-tool store (`bot_tool.rs`) — the latter
closes the known gap where explicit stores bypass `secret_scan` and `detect_prompt_injection`
entirely.

Reject (with a per-rule `tracing` counter, e.g. `memory_reject{rule="bare-number"}`):

| Rule | Test |
|---|---|
| `secret` | `secret_scan::contains_secret(value)` |
| `injection` | `detect_prompt_injection(key or value)` |
| `bare-number` | trimmed value is numeric/boolean and < 12 chars (`23`, `true`, `98.1%`) |
| `time-fragment` | value < 30 chars matching time/date shapes (`8:00 AM`, `April 14, 2026`) |
| `path` | value is a filesystem path (`/Users/…`, `/tmp/…`, `C:\…`, app-data dirs) |
| `key-blocklist` | key ∈ {`current-date`, `date`, `time`, `timestamp`, `tool-usage-count`, `input-format`, `input-file-path`, `message-count`, …} |
| `echo` | `normalize(key) == normalize(value)` |
| `too-thin` | value ≤ 2 words AND `explicit != true` (explicit `user/favorite-color: blue` survives) |

Filters are pure functions in `memory.rs` with table-driven tests. Audit baseline they must kill:
115 bare numbers/bools, 61 time fragments, 17 paths, 13 echoes in the dev corpus.

## Consolidation alignment (existing `memory_consolidation.rs`)

Gate chain unchanged (enabled → 24h/scope → ≥20 memories → scope lock). Prompt gains:

- **Durable-vs-dated test:** preferences/relationships/recurring workflows are durable — sharpen
  them; topic memories whose work is done or date has passed are dated — delete, folding any
  lasting takeaway into a durable memory.
- **Merge rule:** when combining duplicates, keep the richer value and the **oldest** `created_at`.
- **Topic retirement** is the only expiry mechanism in the system.

**One-time legacy sweep** (deterministic, before the LLM pass, per scope): delete ALL `daily/*`
entries. No legacy support needed (45 beta users, 3 active — decision 2026-06-12); the layer is
retired outright. Removes the 710-entry dev graveyard and prod's dailies in one pass.

## Deliberate-store path (prompt change only)

Current memory docs say "you do NOT need to call store." Soften to:

> Auto-capture handles most memory. Store **immediately** via the memory tool in two cases:
> the user explicitly asks you to remember something, or the user corrects how you work
> (corrections are the highest-value memories — include the why).

Two triggers, one storage op (`upsert_memory`) — not a competing pathway. Same shape as Claude's
auto + deliberate writes. Corrections are what the sidecar most reliably fumbles (they look like
ordinary dialogue); the main agent has the salience.

## Deferred (own rounds, after measurement)

- **Recall selector** — Claude-style: small model gets the memory index (key + value first line)
  + the user prompt at turn start, runs in parallel, surfaces picks via the existing Reminder
  stream (`<system-reminder>`) mid-turn. No critical-path latency. Land only after the corpus is
  clean so its value is measurable. (Claude also has a `synthesize` mode — many tiny memories →
  one authored paragraph — worth considering at the same time.)
- **Write-side judge** — only if post-R1 harness runs show residual semantic junk the prompt +
  filters can't catch. Design if needed: one batched call per extraction, tiny cached system
  prompt, verdicts `keep | demote | reject` + ≤8-word reason.
- **`reference` layer** — if topic facts holding URLs/dashboards prove awkward.

## Rollout — one measurable change per round

| Round | Change | Metric |
|---|---|---|
| R1 | Extraction prompt v2 + category collapse + stage-0 filters (both write paths) | junk-signature rate on new writes (audit queries re-run); per-rule reject counters; facts/turn |
| R2 | `agent.json` topics + per-scope prompt assembly + tool `layer` validation + manifest validation | topic-classified share of new memories in a topic-declaring agent; manual quality read |
| R3 | Consolidation prompt alignment + one-time daily sweep | corpus size before/after; dailies remaining = 0; spot-check retired topics |
| R4 | Deliberate-store prompt change | explicit stores/week; correction-class memories captured |

Test live against Janus (no mock mode), session IDs timestamped, per the testing doctrine.

## Rollout progress

- [x] R1 — extraction prompt v2 + category collapse + stage-0 filters (both write paths) —
  **shipped 2026-06-12.** Metric: 17/17 audit junk signatures rejected by stage-0 table tests
  (bare numbers/bools, time fragments, paths, blocklisted keys, echoes, too-thin); explicit-store
  bypass closed (`bot_tool` now runs `stage0_reject`); `nebo-agent` 304 + `nebo-tools` 232 tests
  green. Per-rule reject counters live via `tracing` (`memory_reject` warn lines) — live junk-rate
  on new writes to be read after real usage. Bonus (no-legacy decision): tool `daily` layer removed,
  `project` layer added to tool schema.
- [x] R2 — agent.json topics + per-scope prompt assembly + tool layer validation —
  **shipped 2026-06-12.** Metric: topic-classified share to be read from a topic-declaring agent's
  new writes once one ships; structural verification green — `MemoryTopic` parse/validation tests
  (reserved slugs, kebab-case, 8-topic cap, 120-char descriptions all rejected), declared-slug
  storage mapping test (`lead` kept, undeclared → `project`), extraction prompt assembles declared
  topic lines per scope (runner + pre-compaction flush both threaded), memory tool `layer` accepts
  declared slugs via `ToolContext.memory_topics`. `nebo-napp` 127 + `nebo-agent` 305 + `nebo-tools`
  231 green (pre-existing `shell_tool` parallel-load flake passes in isolation).
  **Follow-up (cross-repo):** NeboLoop scanner must accept/validate `memory.topics` in agent
  manifests — not a blocker, desktop validation rejects invalid configs at load.
- [x] R3 — consolidation prompt alignment + one-time daily sweep — **shipped + live-verified
  2026-06-12.** Live metric: `daily/*` count 16 → **0** in the live DB
  (`~/Library/Application Support/Nebo/data/nebo.db` — confirmed via `lsof` as the DB the dev
  process opens; the `Nebo-Dev/data` DB named in earlier audits is an orphaned April-era copy no
  process opens — its 710 dailies sweep automatically if an instance ever opens it, since the
  retirement sweep runs at every startup). Post-sweep namespaces are exactly the invariant +
  topical layers. Consolidation now loads the WHOLE scope (was tacit/-only — topic
  layers were invisible to the only reaper); prompt gains durable-vs-dated test, done/dated topic
  retirement, and merge-to-oldest-id rule (preserves original created date). Startup sweep retires
  ALL `daily/*` rows across scopes (idempotent SQL, before the LLM loop). Unit test green
  (cross-scope delete, tacit untouched); `nebo-db` 6 + `nebo-agent` 305 + `nebo-tools` 231 green.
  **Remaining check:** dev DB daily count (710) → 0 once `cargo tauri dev` finishes rebuilding and
  restarts the backend; prod (16) clears on next prod app launch with this build. Verify next
  iteration, then check this box.
- [x] R4 — deliberate-store prompt change — **shipped 2026-06-12.** The `## Memory` section of
  the static prompt now states the two immediate-store triggers verbatim (explicit "remember"
  requests; behavioral corrections, with the why) over the auto-capture floor, replacing the
  softer "proactively save" framing. Declarative-facts guidance retained. Metric (explicit
  stores/week, correction-class capture rate) reads from live usage going forward.
  `nebo-agent` 305 + `nebo-tools` 231 green (the recurring `shell_tool::plain_commands_still_execute`
  parallel-load flake passes in isolation every time — pre-existing, worth a separate fix).

## Known adjacent issues (not in scope, tracked)

- Companion scope inconsistency: dev rows with empty owner prefix (`:agent:x`, ``) vs prod
  (`uuid:agent:x`), and the `:agent:assistant` scope. Needs its own fix before cross-scope
  features lean on `memory_scope_chain`.
- Explicit tool stores don't trigger `embed_memories_async` (extraction-path stores do).
- `docs/sme/MEMORY_AND_PROMPT.md` is stale in 3 areas (see 2026-06-12 verification) — update
  alongside R1.

## File change map

| File | Change |
|---|---|
| `crates/agent/src/memory.rs` | prompt v2 + `{TOPIC_CATEGORIES}` assembly; `ExtractedFacts` (drop `decisions`/`task_context`, add `topic` field); `format_for_storage` v2; stage-0 filter fns + tests |
| `crates/tools/src/bot_tool.rs` | stage-0 filters + secret/injection on explicit store; `layer` accepts declared topic slugs |
| `crates/napp/src/agent.rs` | `MemoryTopic` + `MemoryConfig.topics` + validation |
| `crates/agent/src/runner.rs` | thread the scope's topics into extraction context |
| `crates/agent/src/memory_consolidation.rs` | prompt alignment; one-time daily sweep |
| `crates/agent/src/prompt.rs` | memory-docs section: deliberate-store guidance |
| NeboLoop scanner (separate repo) | accept/validate `memory.topics` in agent manifests |
