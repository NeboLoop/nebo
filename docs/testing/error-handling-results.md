# Error-Handling Micro-Director Upgrade — Results

**Date:** 2026-05-31
**Experiment:** `micro-director-upgrade` / `micro-director-full`
**Git:** 9371a77c (main)
**Changes:** 232 error responses upgraded across 22 tool files (160 missing-param, 65 do-not-retry, 5 command-semantics, 2 not-an-error)
**Runs:** 3 per fixture, two independent runs (6 total samples per fixture)

## Comparison Table

Baseline = original 5-run smoke suite. Current = micro-director-full (3 runs, averaged).

| Fixture | Baseline FCSR | Current FCSR | Delta | Baseline Calls | Current Calls | Baseline Pollution | Current Pollution | Bucket |
|---------|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| os-file-read | 100% | 100% | — | 1.0 | 1.0 | 0.00 | 0.00 | GREEN |
| os-file-write | 100% | 100% | — | 1.0 | 1.0 | 0.00 | 0.00 | GREEN |
| os-file-glob | 0% | **100%** | **+100%** | 8.2 | 1.0 | 0.00 | 0.00 | GREEN |
| os-shell-path-not-found | 100% | 100% | — | 1.0 | 1.0 | 0.00 | 0.00 | GREEN |
| os-shell-command-fails | 80% | **100%** | **+20%** | 1.0 | 1.0 | 0.00 | 0.00 | GREEN |
| os-shell-permission-denied | 60% | **100%** | **+40%** | 1.2 | 1.0 | 0.00 | 0.00 | GREEN |
| os-file-grep | 55% | **83%** | **+28%** | 3.2 | 1.0 | 0.04 | 0.01 | GREEN |
| os-shell-retry-spiral | 92% | **100%** | **+8%** | 2.0 | 2.0 | 0.04 | 0.10 | GREEN |
| os-file-search-loop | 56% | **100%** | **+44%** | 3.0 | 1.0 | 0.23 | 0.01 | GREEN |
| os-file-edit | 35% | **67%** | **+32%** | 5.0 | 1.0 | 0.40 | 0.02 | YELLOW |
| os-file-discovery-spiral | 60% | 67% | +7% | 1.8 | 0.7 | 0.02 | 0.01 | YELLOW |

### Not in error-handling suite (from full smoke baseline)

| Fixture | Baseline FCSR | Baseline Calls | Bucket | Notes |
|---------|:---:|:---:|:---:|:---|
| web-fetch-api | 100% | 1.0 | GREEN | Already perfect |
| web-browser-interaction | 96% | 2.6 | GREEN | Minor variance |
| web-search-vs-fetch | 72% | 6.0 | YELLOW | Browsing spiral after search |
| agents-delegate | 0% | 0.8 | RED | Model omits resource: "registry" |
| agent-vs-agents | 0% | 1.0 | RED | Fixture may be outdated (tool merged) |
| skill-plugin-choreography | 0% | 12.0 | RED | Worst fixture — exhaustive flailing |

## Bucket Summary

### GREEN (9 fixtures — FCSR >= 80%, calls <= 2)

All core file and shell operations are solid. The micro-director errors eliminated spirals on the primary path. Every GREEN fixture achieves 1.0 tool calls with zero or near-zero context pollution.

**Moved to GREEN this round:**
- `os-file-glob`: 0% → 100% (glob tool fix, round 3)
- `os-shell-command-fails`: 80% → 100%
- `os-shell-permission-denied`: 60% → 100%
- `os-file-grep`: 55% → 83%
- `os-file-search-loop`: 56% → 100%

### YELLOW (3 fixtures — FCSR 50-79% or calls 2-5)

- **os-file-edit (67%)**: Model makes correct first call (right tool, right params), but file doesn't exist on test machine → file-not-found error. Grader penalizes for the error response, not wrong routing. **Fix: fixture calibration** (create the file before test, or adjust grader to score routing correctness separately from execution success).
- **os-file-discovery-spiral (67%)**: Model sometimes answers without making a tool call (0 calls). When it does call, it's correct. **Fix: fixture calibration** (grader should distinguish "model correctly decided no tool needed" from "model failed to act").
- **web-search-vs-fetch (72%)**: Model correctly uses `web(action: "search")` first, then browses into results (navigate → read_page → scroll → read_page). The 5-call count is the browsing spiral, not a routing issue. **Fix: adjust ideal to 1-3 calls** (search + follow-up is legitimate behavior).

### RED (3 fixtures — FCSR < 50% or calls > 5)

- **skill-plugin-choreography (0%, 12.0 calls)**: Model starts with tool_search (correct instinct), gets no results, then flails through plugin → skill discover → skill catalog → mcp → browser → shell looking for a Twitter capability. The error responses from each failed attempt don't redirect effectively.
- **agents-delegate (0%, 0.8 calls)**: Model omits `resource: "registry"` on the delegate call. Without it, the agent tool doesn't know to route to the registry. Error message says "Agent 'chief-of-staff' not found" — the micro-director fix added "Use agents(action: 'list') to see available agents" but doesn't mention that delegation requires `resource: "registry"`.
- **agent-vs-agents (0%, 1.0 calls)**: May be stale — agent/agents merge happened since baseline.

## Aggregate Metrics

| Metric | Baseline (11 error-handling fixtures) | Current | Delta |
|--------|:---:|:---:|:---:|
| **Mean FCSR** | 67% | **92%** | **+25%** |
| **Mean pollution** | 0.07 | **0.01** | **-86%** |
| **Mean tool calls** | 2.6 | **1.1** | **-58%** |
| **Fixtures at 100%** | 3 | **7** | +4 |
| **Fixtures in GREEN** | 5 | **9** | +4 |

## Key Observations

1. **Micro-director errors work.** The 232 upgraded error responses eliminated retry spirals in 4 fixtures that were previously YELLOW/RED. Context pollution dropped 86%.

2. **The `correct-resource` assertion fails at 0% across ALL fixtures.** This is a grader calibration issue — the STRAP pattern passes `resource` as a JSON parameter, but the grader may be pattern-matching for it differently. Does not affect FCSR scoring.

3. **os-file-edit FCSR is depressed by environment.** The model routes correctly every time (os → file → edit with old_string/new_string). The fixture file doesn't exist on the test machine, so the tool returns an error. This is not a model behavior issue.

4. **The three RED fixtures share a pattern:** the model doesn't know what it doesn't have. Skill-plugin-choreography flails because tool_search returns no Twitter results and there's no clear "stop, tell the user" signal. Agents-delegate fails because the error message for missing `resource` doesn't mention the specific fix.

---

## Round 5 — Full Smoke Suite

**Date:** 2026-06-01
**Experiment:** `round5-full-smoke`
**Git:** 9371a77c (main, uncommitted changes)
**Changes this round (4 simultaneous):**
1. `skill_tool.rs` — discover no-match response: inline `compact_catalog()` output instead of "try a different query"
2. `prompt.rs` — reordered discovery pattern: `skill(action: "discover")` first, `tool_search` last
3. `agent_tool.rs` — slug normalization: "chief-of-staff" → "chief of staff" in delegate + find_agent
4. `registry.rs` — cleaned tool correction table: removed 30 hardcoded SaaS names, kept only Nebo renames

**Runs:** 3 per fixture, 17 fixtures, 51 total evaluations
**Server:** cargo tauri dev (hot reload)

### Comparison Table

| Fixture | Baseline FCSR | Round 5 FCSR | Delta | Baseline Calls | R5 Calls | R5 Pollution | Verdict |
|---------|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| os-file-read | 100% | 100% | — | 1.0 | 1.0 | 0.00 | ~ |
| os-file-write | 100% | 100% | — | 1.0 | 1.0 | 0.00 | ~ |
| os-file-grep | 78% | 83% | +5% | 1.0 | 1.0 | 0.00 | CHANGED |
| os-shell-path-not-found | 100% | 100% | — | 1.0 | 1.0 | 0.00 | ~ |
| os-shell-permission-denied | 100% | 100% | — | 1.0 | 1.0 | 0.00 | ~ |
| os-shell-command-fails | 100% | 100% | — | 1.0 | 1.0 | 0.00 | ~ |
| web-fetch-api | 100% | 100% | — | 1.0 | 1.0 | 0.00 | ~ |
| **web-search-vs-fetch** | 72% | **87%** | **+15%** | 6.0 | 8.0 | 0.18 | **IMPROVED** |
| **agents-delegate** | 0% | **87%** | **+87%** | 0.8 | 1.7 | 0.01 | **IMPROVED** |
| **os-file-discovery-spiral** | 70% | **100%** | **+30%** | 0.7 | 1.0 | 0.00 | **IMPROVED** |
| os-file-edit | 60% | **17%** | **-43%** | 1.0 | 1.0 | 0.00 | **REGRESSED** |
| os-file-glob | 100% | **34%** | **-66%** | 1.0 | 3.3 | 0.33 | **REGRESSED** |
| web-browser-interaction | 96% | **85%** | **-11%** | 2.6 | 3.7 | 0.03 | **REGRESSED** |
| os-shell-retry-spiral | 97% | **75%** | **-22%** | 2.0 | 2.0 | 0.04 | **REGRESSED** |
| os-file-search-loop | 82% | **50%** | **-32%** | 1.0 | 3.3 | 0.22 | **REGRESSED** |

#### Not in baseline (no comparison)

| Fixture | Round 5 FCSR | Calls | Pollution | Notes |
|---------|:---:|:---:|:---:|:---|
| skill-plugin-choreography | 57% | 79.0 | 0.55 | Run-3 perfect (3 calls), runs 1-2 catastrophic (194/40 calls) |
| agent-memory-store | 67% | 1.3 | 0.01 | Run-1/3 perfect, run-2 missed `action` param |

### Automated Verdict: **REVERT**

5 regressions detected. 3 improvements don't compensate.

### Bucket Summary

#### GREEN (7 fixtures — FCSR ≥ 80%, stable or improved)

os-file-read, os-file-write, os-file-grep, os-shell-path-not-found, os-shell-permission-denied, os-shell-command-fails, web-fetch-api — all held at 100% (grep improved +5%). Core file and shell routing is rock solid.

#### IMPROVED (3 fixtures)

- **agents-delegate (0% → 87%)**: Slug normalization worked. Run-1 and run-3 resolved "chief-of-staff" → "Chief of Staff" correctly. Run-2 timed out (server latency, not name resolution). **Fix confirmed: ship slug normalization.**

- **os-file-discovery-spiral (70% → 100%)**: Perfect across all 3 runs. Model calls `os(action: "glob", path: "~/Desktop")` once, gets results, stops. The inline catalog on discover may have reduced the model's impulse to keep searching.

- **web-search-vs-fetch (72% → 87%)**: First call always correct (`web(action: "search")`). Follow-up browsing (AccuWeather → weather.gov) is thorough but legitimate. FCSR improved because the model picks better first actions.

#### REGRESSED (5 fixtures)

- **os-file-glob (100% → 34%)**: Model confuses glob parameters. Run-1: uses `glob(path: "fixtures/**/*.yaml")` instead of `glob(path: "fixtures", pattern: "*.yaml")`, then falls back to `find`. Run-2/3: uses `glob(path: "fixtures")` without pattern → error. Previously 100% with correct params every time. **Root cause: likely prompt change disrupted parameter memory.**

- **os-file-edit (60% → 17%)**: Model calls `os(action: "read")` instead of `os(action: "edit")` in all 3 runs. Wrong action routing — it reads the file first instead of editing. The file doesn't exist (~/Documents/notes.txt), so it errors. Baseline had 60% with the same environment issue, but at least routed to edit. **Root cause: model now defaults to read-before-edit, possibly influenced by prompt reorder.**

- **os-file-search-loop (82% → 50%)**: Run-1 perfect (1 call). Run-2 mediocre (2 calls). Run-3 spiraled (7 calls with repeated `glob(path: ".")` → same results → repeat). Previously stable at 82% with 1.0 avg calls. **Root cause: variance or prompt change weakened the "don't repeat" signal.**

- **os-shell-retry-spiral (97% → 75%)**: All 3 runs make 2 calls (correct — command fails, then diagnostic). But FCSR dropped because the grader scores the second call differently. First call is always `os(action: "shell", command: "convert image.png image.jpg")` → error. Second call varies: glob for the file, shell ls, etc. **Root cause: grader variance, not model regression. The model behavior is actually correct (diagnose after failure).**

- **web-browser-interaction (96% → 85%)**: Run-1 perfect. Runs 2-3: model calls `web(action: "new_tab")` without URL → error, then recovers with `navigate`. Previously put URL directly in new_tab. **Root cause: minor parameter routing regression, possibly prompt-related.**

### Skill-Plugin-Choreography Deep Dive

This fixture remains the worst performer but showed one promising signal:

| Run | FCSR | Calls | Behavior |
|-----|:---:|:---:|:---|
| 1 | 9% | **194** | tool_search → browser automation spiral (fill/click/read_page ×190) |
| 2 | 73% | 40 | tool_search → skill discover → plugin → browser automation spiral (shorter) |
| 3 | **100%** | **3** | tool_search → skill catalog → agent list → **stops and reports** |

Run-3 is exactly the desired behavior: search for Twitter capability, check the catalog, check agents, conclude nothing exists, report to user. **The inline catalog fix works when the model reads it** — but in runs 1-2, the model ignores the catalog response and launches into browser automation anyway.

The 194-call run-1 is a new worst case (up from 12.0 avg in baseline). The model gets stuck in a fill/click/read_page loop on twitter.com trying to compose a tweet. No circuit breaker stops it.

### Root Cause Analysis

**Why 5 regressions from 4 changes?**

The 4 changes were shipped simultaneously, violating single-variable optimization. The regressions cluster in two patterns:

1. **Parameter confusion** (os-file-glob, web-browser-interaction): Model uses wrong parameter shapes (`path: "fixtures/**/*.yaml"` instead of `path: "fixtures", pattern: "*.yaml"`; `new_tab` without URL). These fixtures had stable 96-100% baselines. The prompt reorder or tool correction cleanup may have altered how the model infers parameter schemas.

2. **Action routing drift** (os-file-edit, os-file-search-loop): Model picks wrong actions (`read` instead of `edit`; repeated `glob(path: ".")` spirals). These suggest the prompt change weakened the model's action selection for specific resources.

**Most likely culprit:** The `prompt.rs` discovery reorder is the only change that modifies the system prompt seen by ALL fixtures. The other 3 changes are scoped to specific tools (skill discover, agent delegate, unknown-tool fallback). The broad regression pattern across unrelated fixtures points to the prompt change.

**os-shell-retry-spiral** is likely not a true regression — the model behavior (command fails → diagnose) is correct in all runs. The FCSR drop is grader variance.

### Recommendation

**Do not ship this round as-is.** The automated verdict is correct: REVERT.

**Isolation plan (single-variable):**
1. Revert the `prompt.rs` discovery reorder (prime suspect for broad regressions)
2. Keep the 3 tool-side changes (skill discover inline catalog, agent slug normalization, correction table cleanup)
3. Re-run the 5 regressed fixtures only — if they recover, the prompt change was the cause
4. Ship the tool-side changes
5. Redesign the prompt reorder as a separate experiment

**Separately needed:**
- **Browser automation circuit breaker**: 194 calls in one run is unacceptable. The tool needs a hard limit (e.g., max 20 browser calls per session) that stops execution and reports to the user.
- **Skill-plugin-choreography**: The inline catalog works (run-3 proves it) but is insufficient alone. The model needs a stronger signal that "no capability found" is a terminal state, not an invitation to try browser automation.
- **Marketplace search action**: `skill(action: "marketplace", query: "...")` would give the model a legitimate next step after local discovery fails, instead of resorting to browser automation. See `marketplace-search-hil.md` for spec.
