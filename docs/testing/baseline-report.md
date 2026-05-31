# Baseline Report — Prompt Test Harness v0.10.2

**Date:** 2026-05-31
**Branch:** feat/memory-sme-hermes-comparison
**Model:** Default (Janus-routed)
**Grader:** claude-sonnet-4-6 (via Claude Code CLI)
**Runs per fixture:** 5
**Fixtures tested:** 16/17 (skill-plugin-choreography timed out)

## Per-Fixture Results

| Fixture | FCSR (mean) | FCSR (range) | Pollution | Tool Calls | Tokens | Assertion Pass % | Bucket |
|---------|-------------|--------------|-----------|------------|--------|------------------|--------|
| os-file-read | 100% | 100-100% | 0.00 | 1.0 | 50,106 | 100% | GREEN |
| os-file-write | 100% | 100-100% | 0.00 | 1.0 | 50,264 | 100% | GREEN |
| os-shell-path-not-found | 100% | 100-100% | 0.00 | 1.0 | 50,316 | 100% | GREEN |
| os-shell-permission-denied | 100% | 100-100% | 0.00 | 1.2 | 56,783 | 100% | GREEN |
| os-shell-command-fails | 100% | 100-100% | 0.00 | 1.0 | 54,209 | 100% | GREEN |
| web-fetch-api | 100% | 100-100% | 0.00 | 1.0 | 56,470 | 100% | GREEN |
| web-browser-interaction | 96% | 80-100% | 0.00 | 2.6 | 92,641 | 80% | GREEN |
| os-shell-retry-spiral | 92% | 75-100% | 0.04 | 2.0 | 89,543 | 77% | GREEN |
| web-search-vs-fetch | 72% | 50-100% | 0.17 | 6.0 | 271,801 | 72% | YELLOW |
| os-file-discovery-spiral | 60% | 50-100% | 0.02 | 1.8 | 99,874 | 97% | YELLOW |
| os-file-search-loop | 56% | 12-100% | 0.23 | 3.0 | 129,192 | 70% | YELLOW |
| os-file-grep | 55% | 33-75% | 0.04 | 3.2 | 158,023 | 79% | YELLOW |
| os-file-edit | 35% | 0-50% | 0.40 | 5.0 | 190,636 | 74% | RED |
| agents-delegate | 0% | 0-0% | 0.34 | 0.8 | 36,599 | 27% | RED |
| os-file-glob | N/A | — | N/A | 8.2 | 301,500 | 0% | RED |
| agent-vs-agents | N/A | — | N/A | 1.0 | 40,385 | 0% | RED |
| skill-plugin-choreography | — | — | — | — | — | — | UNTESTED |

## Buckets

### GREEN (90%+ FCSR) — 8 fixtures
Regression guards. These work reliably:
- os-file-read, os-file-write, os-shell-path-not-found, os-shell-permission-denied, os-shell-command-fails, web-fetch-api, web-browser-interaction, os-shell-retry-spiral

### YELLOW (50-90% FCSR) — 4 fixtures
Intermittent failures. Prompt clarity issues:
- web-search-vs-fetch (72%), os-file-discovery-spiral (60%), os-file-search-loop (56%), os-file-grep (55%)

### RED (<50% FCSR) — 4 fixtures
Consistently failing. First optimization targets:
- os-file-edit (35%), agents-delegate (0%), os-file-glob (N/A — grading failed, 8.2 tool calls), agent-vs-agents (N/A — grading failed)

## RED Fixture Diagnoses

### os-file-edit (FCSR: 35%)
**Problem type:** Tool-side + prompt-side

**Root cause:** The file read tool returns "File unchanged since last read. Contents are already in your conversation context" even in fresh eval sessions where no prior read exists. This confuses the model, which then:
1. Tries to read the file (gets cached non-response)
2. Attempts the edit (fails because file content doesn't match expected)
3. Falls back to shell commands (`cat`, `sed`)

**Consistently failing assertions:**
- `single-call` (5/5 failures) — always requires 4-7 tool calls instead of 1
- `no-sed-fallback` (4/5 failures) — resorts to shell after os(edit) fails

**STRAP doc involved:** os (file resource, edit action)

**Hypothesis:** The file read caching logic needs a session-awareness check — first read in a session should always return actual content. Alternatively, the edit action should not require a prior read to succeed.

### agents-delegate (FCSR: 0%)
**Problem type:** Prompt-side (tool naming confusion)

**Root cause:** The model does NOT call the `agents` tool (plural) for delegation. Instead it either:
- Calls `agent` (singular) which is for self-management (memory, tasks, sessions)
- Responds conversationally without any tool call

The STRAP docs define two separate tools: `agent` (self-management) and `agents` (multi-agent delegation). The model consistently confuses them.

**Consistently failing assertions:**
- `uses-agents-not-agent` (5/5 failures)
- `correct-action` (5/5 failures)
- `has-name` (5/5 failures)
- `no-spawn` (4/5 failures)

**STRAP doc involved:** agents (delegate resource)

**Hypothesis:** The naming collision between `agent` and `agents` is fundamentally confusing. The model sees "agent" in the prompt and defaults to the singular form. Either rename the delegation tool to something distinct (e.g., `delegate` or `team`) or add a prominent disambiguation rule in the STRAP preamble.

### os-file-glob (FCSR: N/A, 8.2 tool calls avg)
**Problem type:** Tool-side + prompt-side

**Root cause:** Two issues compound:
1. The model passes the glob pattern in `path` instead of `pattern` (argument confusion)
2. Even when the model correctly uses `pattern`, the glob tool returns "No files found" for every attempt — the tool's path resolution appears broken in eval sessions (working directory mismatch)

The model then spirals: trying different pattern syntaxes, falling back to shell `find`, eventually timing out.

**STRAP doc involved:** os (file resource, glob action)

**Hypothesis:** The glob tool's argument documentation needs to clearly distinguish `path` (search root directory) from `pattern` (glob expression). Additionally, the glob implementation may have a working directory bug — the `fixtures/` directory exists in the repo root but the eval session may execute from a different cwd.

### agent-vs-agents (FCSR: N/A, grading failed)
**Problem type:** Likely prompt-side (same as agents-delegate)

**Root cause:** Same naming collision issue. The grader couldn't produce valid JSON for this fixture, but the raw metrics show 1.0 tool calls on average with 0% assertion pass rate — suggesting the model calls `agent` (singular) instead of correctly routing to the appropriate tool.

**STRAP doc involved:** agent / agents disambiguation

**Hypothesis:** Same fix as agents-delegate — the `agent` vs `agents` naming is the core problem.

## Consistently Failing Assertions (3+ failures out of 5 runs)

| Fixture | Assertion | Fail Rate |
|---------|-----------|-----------|
| agents-delegate | uses-agents-not-agent | 5/5 |
| agents-delegate | correct-action | 5/5 |
| agents-delegate | has-name | 5/5 |
| os-file-edit | single-call | 5/5 |
| web-search-vs-fetch | max-three-calls | 5/5 |
| agents-delegate | no-spawn | 4/5 |
| os-file-edit | no-sed-fallback | 4/5 |
| os-file-grep | single-call | 4/4 |
| web-browser-interaction | uses-navigate | 4/5 |
| os-file-search-loop | correct-tool | 3/5 |
| os-file-search-loop | single-call | 3/5 |
| os-shell-retry-spiral | correct-tool | 3/5 |
| os-shell-retry-spiral | suggest-install-or-alternative | 3/5 |

## Overall Statistics

| Metric | Value |
|--------|-------|
| Aggregate FCSR | 75% (n=67 graded runs) |
| Aggregate context pollution | 0.093 |
| Average tokens per fixture | 108,021 |
| Average tool calls per fixture | 2.5 |
| STRAP doc in most RED fixtures | `os` (file resource — edit, glob) and `agents` (delegation) |
| System prompt size | ~20,222 chars (~5,055 tokens) — uniform across all fixtures |

## Observations

1. **Simple operations work perfectly.** File read, file write, shell commands with clear error messages all achieve 100% FCSR. The STRAP pattern works when the action is unambiguous.

2. **The `agent` vs `agents` naming collision is the single worst prompt-side issue.** 0% FCSR on delegation tasks because the model cannot distinguish the two tools by name alone.

3. **File operations that require multiple arguments (edit, glob, grep) degrade.** The model often gets the first argument right but confuses which parameter holds which value. This is a STRAP doc clarity issue.

4. **The `max-three-calls` assertion fails universally for web-search-vs-fetch.** The model uses 6 tool calls on average because web(search) → web(navigate) → web(read_page) is 3 calls minimum, and real searches often require following links.

5. **Grading through the Nebo chat pipeline was broken** (model responded conversationally instead of returning JSON). Fixed by routing all grading through the Claude Code CLI directly.

6. **Server crashes under sustained load** — the server died during the full 85-run suite, requiring fixture re-runs. Not a prompt issue but affects harness reliability.

## Trace Location

All traces saved to: `.nebo/test-results/baseline/`
Format: `{fixture_id}_run-{n}.json`
