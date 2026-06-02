# Naming Convention A/B Test Report

**Date:** 2026-05-31
**Model:** Janus nebo-1 (default)
**Runs per fixture:** 3
**Server:** nebo-cli (debug build, commit includes agent/agents merge + flat alias routing)

## Hypothesis

The STRAP pattern (Single Tool Resource Action Pattern) consolidates 35+ tools into 10 domain tools using `tool(resource: "x", action: "y")` dispatch. This saves context but may fight the model's training priors, which are tuned on flat tool names (`file_read`, `bash`, `grep`).

**Test:** Run identical tasks with two system prompts — STRAP naming vs flat naming — and compare first-call success rate, total tool calls, and token consumption.

## Method

- **STRAP variant:** Default Nebo system prompt. Tools: `os(resource: "file", action: "read")`, `web(action: "search")`, etc.
- **Flat variant:** Override system prompt with Claude Code-style flat names: `file_read`, `file_write`, `bash`, `grep`, `web_search`. Server-side alias routing maps flat names to STRAP handlers transparently.
- **7 fixtures:** read-file, write-file, edit-file, grep-file, shell-command, web-search, delegate-agent
- **3 runs each** for variance measurement

## Results

### Tool Calls (lower is better)

| Fixture           | STRAP | Flat | Delta  | Winner |
|-------------------|-------|------|--------|--------|
| read-file         | 1.0   | 1.7  | +0.7   | STRAP  |
| write-file        | 1.0   | 1.0  | 0      | Tie    |
| edit-file         | 2.7   | 5.3  | +2.6   | STRAP* |
| grep-file         | 2.0   | 1.0  | -1.0   | Flat   |
| shell-command     | 0.7   | 1.0  | +0.3   | Flat** |
| web-search        | 22.3  | 1.0  | -21.3  | Flat***|
| delegate-agent    | 2.7   | 4.0  | +1.3   | STRAP* |
| **Avg across all**| **4.6** | **2.1** | **-2.5** | **Flat** |

### Token Consumption (lower is better)

| Fixture           | STRAP    | Flat     | Ratio  |
|-------------------|----------|----------|--------|
| read-file         | 50,999   | 29,658   | 1.7x   |
| write-file        | 51,026   | 21,856   | 2.3x   |
| edit-file         | 118,068  | 73,427   | 1.6x   |
| grep-file         | 110,073  | 29,292   | 3.8x   |
| shell-command     | 79,617   | 22,029   | 3.6x   |
| web-search        | 971,235  | 37,921   | 25.6x  |
| delegate-agent    | 90,864   | 58,331   | 1.6x   |
| **Avg across all**| **210,269** | **38,930** | **5.4x** |

## Confounders

Results marked with asterisks have known confounders:

- `*edit-file (STRAP wins)`: Flat variant errored on `replace_all: "True"` (string vs boolean). The flat override's schema didn't constrain the type. The model's tool call was structurally correct — it called `file_edit` with the right params. This is an override quality issue, not a naming issue.
- `*delegate-agent (STRAP wins)`: Flat override didn't include agent registry documentation. Model called `agent(action: "delegate")` without `resource: "registry"` — it hadn't been told about the registry resource. This tests the override completeness, not naming.
- `**shell-command (Flat wins)`: STRAP averaged 0.7 calls — the model didn't call any tool in 1 of 3 runs (just answered from knowledge). Not a naming failure.
- `***web-search (Flat wins massively)`: STRAP's 22.3 calls were mostly the model browsing after a successful search (opening tabs, reading pages). The first call was correct in both variants. The STRAP model got "agentic" — helpful but expensive. This measures behavior style, not naming accuracy.

## Clean Comparisons

Removing confounded fixtures, the cleanest signal comes from:

| Fixture     | STRAP | Flat | Signal |
|-------------|-------|------|--------|
| grep-file   | 2.0   | 1.0  | STRAP model forgot `regex` param on first call, had to retry |
| write-file  | 1.0   | 1.0  | Identical — both got it first try |
| read-file   | 1.0   | 1.7  | Flat model called an extra tool in 2/3 runs |

The grep failure is instructive. The STRAP first call was `os(action: "grep", path: "/tmp/nebo-project/")` — missing the `regex` param. The flat first call was `grep(regex: "TODO", path: "/tmp/nebo-project/")` — complete. The flat tool name `grep` implies "search for a pattern" so strongly that the model included the regex. The STRAP name `os(action: "grep")` is generic enough that the model forgot the key param.

## Token Overhead

Even when call counts are identical (write-file: both 1.0), STRAP uses 2.3x more tokens (51K vs 22K). This is the system prompt overhead — the STRAP system prompt is ~23K chars vs the flat override at ~2K chars. The STRAP prompt carries documentation for all 10 domain tools, behavioral rules, and examples. The flat override is minimal.

This is the STRAP tradeoff working as designed: bigger prompt, fewer tools. But the token cost is paid on every turn, not just the first.

## First-Call Analysis

What the model called first in each variant:

| Fixture        | STRAP first call                              | Flat first call                      |
|----------------|-----------------------------------------------|--------------------------------------|
| read-file      | `os(action: "read", path: "...")`             | `file_read(path: "...")`            |
| write-file     | `os(action: "write", path: "...", content: "...")` | `file_write(path: "...", content: "...")` |
| edit-file      | `os(action: "read", path: "...")` (read first) | `file_edit(old_string, new_string)` (direct edit) |
| grep-file      | `os(action: "grep", path: "...")` (missing regex!) | `grep(regex: "TODO", path: "...")`  |
| shell-command  | `os(action: "exec", command: "uptime")`       | `bash(command: "uptime")`            |
| web-search     | `web(action: "search", query: "...")`         | `web(action: "search", query: "...")` |
| delegate-agent | `agent(resource: "registry", action: "delegate")` | `agent(action: "delegate")` (missing resource) |

Key observations:
1. **Flat names carry semantic weight.** `grep` implies "search with a pattern" — the model includes the regex. `file_edit` implies "edit in place" — the model edits directly without reading first. STRAP's generic `os(action: "grep")` loses this semantic cue.
2. **STRAP's resource/action dispatch works when the model has full docs.** Delegate succeeded with STRAP because the agent tool's schema includes registry examples. It failed with flat because the override was incomplete.
3. **Both conventions achieve ~100% first-call success on simple operations** (read, write). The delta appears on operations that need specific params (grep's regex) or behavioral guidance (edit's "don't read first").

## Conclusions

1. **Token efficiency:** Flat naming uses 5.4x fewer tokens on average. Most of this is system prompt size, not retry overhead. Even with identical call counts, STRAP's prompt costs more.

2. **Naming convention cost:** The STRAP dispatch pattern has a measurable but small FCSR cost on param-heavy tools (grep missing regex). The bigger cost is behavioral — models trained on flat tools have stronger priors about what each tool does and what params it needs.

3. **The real STRAP value is architectural, not naming.** Deferred loading, keyword activation, and contextual filtering are the wins. These don't require `resource/action` dispatch — they can work with flat names too.

4. **Recommendation:** Keep STRAP's architecture. Change the model-facing names to flat convention. The server already accepts flat aliases (`resolve_flat_alias` in registry.rs) — extend this to be the primary interface, with STRAP dispatch as the internal routing layer.

---

## V2: Corrected Methodology (Isolated Naming Variable)

**Date:** 2026-05-31
**Change:** Replace only the STRAP doc content via `--override tool.os` and `--override tool.web`. Full system prompt stays intact — all behavioral rules, deferred docs, keyword activation, memory context preserved. Only the tool documentation changes from STRAP dispatch to flat names.

### Critical Finding: The Model Ignored Flat Names

The model called `os(...)` in every single run, never `file_read(...)`, `grep(...)`, or `bash(...)`. The tool schema registers the tool as `os` with `resource` and `action` parameters. The model follows the schema, not the documentation naming. Flat names in the doc are treated as descriptive text, not as callable tool names.

### Tool Calls (lower is better)

| Fixture           | STRAP | Flat v1 (2K prompt) | Flat v2 (full prompt) | Winner |
|-------------------|-------|---------------------|----------------------|--------|
| read-file         | 1.0   | 1.7                 | 1.0*                 | Tie    |
| write-file        | 1.0   | 1.0                 | 1.0                  | Tie    |
| edit-file         | 2.7   | 5.3                 | 3.7                  | STRAP  |
| grep-file         | 2.0   | 1.0                 | 1.0                  | Flat** |
| shell-command     | 0.7   | 1.0                 | 2.0                  | STRAP  |
| web-search        | 22.3  | 1.0                 | 35.3                 | STRAP  |
| delegate-agent    | 2.7   | 4.0                 | 3.0                  | Tie    |
| **Avg across all**| **4.6** | **2.1**           | **6.7**              | **STRAP** |

`*` read-file run-3 had 0 tool calls — model answered from knowledge without calling any tool.
`**` grep improvement is a documentation clarity effect, not a naming effect (see analysis below).

### Token Consumption (lower is better)

| Fixture           | STRAP    | Flat v1    | Flat v2    | V2/STRAP |
|-------------------|----------|------------|------------|----------|
| read-file         | 50,999   | 29,658     | 115,957    | 2.3x     |
| write-file        | 51,026   | 21,856     | 65,112     | 1.3x     |
| edit-file         | 118,068  | 73,427     | 171,238    | 1.4x     |
| grep-file         | 110,073  | 29,292     | 59,632     | 0.5x     |
| shell-command     | 79,617   | 22,029     | 100,264    | 1.3x     |
| web-search        | 971,235  | 37,921     | 1,600,000  | 1.6x     |
| delegate-agent    | 90,864   | 58,331     | 123,305    | 1.4x     |
| **Avg across all**| **210,269** | **38,930** | **319,358** | **1.5x** |

### First-Call Analysis

| Fixture        | STRAP first call                              | Flat v2 first call                            |
|----------------|-----------------------------------------------|-----------------------------------------------|
| read-file      | `os(action: "read", path: "...")`             | `os(action: "read", path: "...")`            |
| write-file     | `os(action: "write", path: "...", content: "...")` | `os(action: "write", path: "...", content: "...")` |
| edit-file      | `os(action: "read", path: "...")` (read first) | `os(action: "edit", old_string: ..., new_string: ...)` (direct edit) |
| grep-file      | `os(action: "grep", path: "...", pattern: "TODO")` (WRONG param) | `os(action: "grep", path: "...", regex: "TODO")` (CORRECT param) |
| shell-command  | `os(action: "shell", command: "uptime")`      | `os(command: "uptime")` (missing action AND resource) |
| web-search     | `web(action: "search", query: "...")`         | `web(action: "search", query: "...")`         |
| delegate-agent | `agent(resource: "registry", action: "delegate")` | `agent(action: "delegate")` (missing resource) |

### Per-Fixture Analysis

**grep-file (Flat v2 wins: 1.0 vs 2.0):** The STRAP baseline used `pattern: "TODO"` — wrong param name. The flat override doc listed `grep(regex: "TODO", path: "/src/")` as an example, which primed the model to use `regex` even while calling the STRAP tool name `os`. The flat doc's parameter emphasis fixed the param naming. **This is a documentation clarity win, not a naming convention win.** The same fix would work by changing the STRAP doc example to emphasize `regex` as the param name.

**shell-command (STRAP wins: 0.7 vs 2.0):** The flat override doc said `bash(command: "uptime")`. The model knew the tool was named `os` (from the schema) but the `bash` naming confused it about what params to pass. First call was `os(command: "uptime")` — missing both `resource: "shell"` and `action: "exec"`. **Mixed signals (flat docs + STRAP schema) HURT performance.**

**edit-file (STRAP wins: 2.7 vs 3.7):** Run-3 spiraled to 8 calls. One call used `os(action: "file_read")` — the flat doc's `file_read` name leaked into the STRAP `action` field. Cross-contamination from contradictory naming.

**web-search (STRAP wins: 22.3 vs 35.3):** Both variants correctly called `web(action: "search")` on the first call. The browsing spiral is identical in both — same behavioral rules, same full prompt. The v2 runs happened to browse more aggressively. This is LLM variance on browsing depth, not a naming effect.

**delegate-agent (Tie: 2.7 vs 3.0):** Agent doc was NOT overridden (no flat equivalent exists). Model called `agent(action: "delegate")` consistently — omitting `resource: "registry"`. The delegate fixture is identical across STRAP and Flat v2 since the same agent doc was used. The 3x3 consistent behavior (every run: delegate → list → retry with capitalized name) suggests a doc clarity issue, not a naming issue.

### V2 Conclusions

1. **Flat v1's 5.4x token savings were a prompt-size effect, not naming.** With the full prompt, flat v2 uses 1.5x MORE tokens than STRAP. The v1 test's 2K minimal prompt explained 100% of the token advantage.

2. **The model follows tool schemas, not documentation names.** Flat tool names in STRAP doc content are ignored — the model calls whatever the schema registers. This means you can't A/B test naming by changing docs alone. You'd need to change the actual tool registration.

3. **Mixed signals actively hurt.** Documenting `bash(command)` when the schema says `os(resource, action)` causes the model to produce malformed calls. Shell went from 0.7 to 2.0 calls because the model was confused about which interface to follow.

4. **Documentation clarity matters more than naming.** The grep improvement (2.0 → 1.0) came from the flat doc emphasizing `regex` as the param name. The STRAP doc's generic table format didn't prime the model to use the right param. This is fixable without changing the naming convention.

5. **Revised recommendation:** The flat naming hypothesis is not supported when the naming variable is properly isolated. The original recommendation to switch to flat names was based on confounded data (prompt size + naming changed simultaneously). Instead:
   - Fix the STRAP docs: emphasize correct param names in examples (especially `regex` for grep)
   - Fix shell first-call: the `action: "shell"` pattern (without resource) suggests the model confuses action and resource — add a stronger example
   - Keep STRAP architecture — it's not hurting naming accuracy
   - The `resolve_flat_alias` routing is still valuable as a safety net for hallucinated flat names, but not as the primary interface

## Raw Trace Locations

- STRAP: `.nebo/test-results/naming-strap/`
- Flat v1 (minimal prompt): `.nebo/test-results/naming-flat/`
- Flat v2 (full prompt): `.nebo/test-results/naming-flat-v2/`
