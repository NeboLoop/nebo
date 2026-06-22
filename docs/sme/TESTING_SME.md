# Testing System — SME Reference

How Nebo is tested at the **runtime/behavioral** level: the two distinct test
systems, how to actually run each one, and the traps that waste an afternoon if
you don't know them.

**Status:** Current | **Last updated:** 2026-06-19

> Scope: this covers the two *behavioral* harnesses that drive the running app —
> the **integration lifecycle plan** and the **prompt/tool-routing harness**. It
> does **not** cover `cargo test` unit tests (those live next to the code).

---

## Table of Contents

1. [The two systems at a glance](#1-the-two-systems-at-a-glance)
2. [System A — Integration lifecycle plan](#2-system-a--integration-lifecycle-plan)
3. [System B — Prompt / tool-routing harness](#3-system-b--prompt--tool-routing-harness)
4. [The grader is hardwired to claude-code](#4-the-grader-is-hardwired-to-claude-code)
5. [CRITICAL gotchas (read before running anything)](#5-critical-gotchas-read-before-running-anything)
6. [Plan drift — where the docs lag the implementation](#6-plan-drift)
7. [File / path reference map](#7-file--path-reference-map)

---

## 1. The two systems at a glance

| | **A. Integration plan** | **B. Prompt harness** |
|---|---|---|
| Lives in | `tests/integration/` | `fixtures/` + `suites/` (repo root) |
| Question it answers | "Does every lifecycle operation actually work end-to-end?" | "Does the model pick the right tool first try, recover from errors, not spiral?" |
| Unit of work | 81 numbered tests (PF, AT, S, T, W, A/R, X) | 45 YAML fixtures grouped into suites |
| Executor | **An agent (you)** driving the running server | `nebo-cli test run` driving the running server |
| Server | **`make dev` must be UP** | **`make dev` must be UP** |
| Output | `tests/integration/results/YYYY-MM-DD-<plat>-NN.md` | FCSR scorecard + trace JSON |
| Grading | Human/agent judgment (observe-and-document) | **claude-code** LLM-as-judge (`--grader`) |

**Both drive the one running server.** Neither boots its own. The single most
common mistake (see §5) is running a binary that *does* boot its own server.

---

## 2. System A — Integration lifecycle plan

`tests/integration/` — a structured, exhaustive, **observe-and-document-only**
walk through every artifact lifecycle operation.

- `plan.md` — the 81 tests, with exact commands + expected results.
- `README.md` — naming + regression-tracking rules.
- `run-prompt.md` — the prompt you hand to the executor (Nebo via `nebo chat -i`,
  or Claude Code). "Execute every test, never fix, record PASS/FAIL/SKIP."
- `results/TEMPLATE.md` — copy to `results/YYYY-MM-DD-<platform>-NN.md`, fill in.
- `results/*.md` — prior runs. Diff against the **most recent same-platform** run
  for regressions.

### How to execute it

`make dev` UP, then for each test:

- **REST tests** → `curl …/api/v1/…` (e.g. `POST /api/v1/codes`, `/api/v1/skills`,
  `/api/v1/workflows`, `/api/v1/agents`).
- **Agent-tool tests** → `POST /agent/mcp` with JSON-RPC `tools/call` — this
  executes a Nebo tool **deterministically, no LLM in the loop**:
  ```
  curl -s -X POST http://localhost:27895/agent/mcp -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"tools/call",
         "params":{"name":"os","arguments":{"resource":"file","action":"read","path":"/tmp/x"}}}'
  ```
  `tools/list` enumerates the exposed tools. Handler: `handlers::mcp_server::agent_mcp_handler`
  (`crates/server/src/handlers/mcp_server.rs`), route mounted at `/agent/mcp`
  (`crates/server/src/lib.rs`). **No auth required** locally.
- **Filesystem checks** → read under `~/Library/Application Support/Nebo/`
  (macOS data dir; *not* `~/.nebo`).

Write results **as you go** (don't batch — a crash loses unsaved rows). Finish with
the Summary table + a regression diff against the previous run.

### `/agent/mcp` has NO agent session

The endpoint runs in a **system tool-context with no active agent**. Read/most ops
work (os, web, memory, event, skill, message). But **agent-scoped *writes* fail**:
workflow create → `"workflow creation must be scoped to an agent"`; persona/agent
create similarly. Do those via REST (`POST /api/v1/workflows`, `/api/v1/agents`) —
which is what the plan's W and A sections already specify.

---

## 3. System B — Prompt / tool-routing harness

Measures **first-call success rate (FCSR)** and recovery quality. Built from:

- `fixtures/**/*.yaml` — one scenario each: a `conversation` (the user turn(s)),
  optional `setup`/`teardown` shell, `tool_config`, `prompt_assertions`
  (`first_call` / `recovery` / `cost`, each `severity: critical|important`), and
  `ideal_behavior`.
- `suites/*.yaml` — a `name:` plus a `fixtures:` list of `../fixtures/...` paths
  (relative to `suites/`, i.e. the **repo-root `fixtures/`**, *not* `tests/fixtures/`).
  Suites: `smoke`, `error-handling`, `naming-{flat,strap}`, `regressed-5`,
  `remaining`, `web-{dynamic,research}`.

### How to run it (the correct, historically-used invocation)

`make dev` UP, then run the **`nebo-cli`** binary (NOT `nebo` — see §5):

```bash
# single fixture
./target/debug/nebo-cli test run \
  --fixture fixtures/tools/os-file-read.yaml \
  --grader claude-sonnet-4-6 --server localhost:27895 \
  --runs 1 --output /tmp/nebo-traces

# a whole suite
./target/debug/nebo-cli test run \
  --suite suites/error-handling.yaml \
  --grader claude-sonnet-4-6 --server localhost:27895 \
  --runs 1 --output /tmp/eh-traces
```

Flags: `--fixture` | `--suite` (one required), `--grader <model>` (enables
LLM-as-judge — see §4), `--runs N` (variance), `--server host:port` (default
`localhost:27895`), `--output DIR` (per-fixture trace JSON, written as each
completes — your live progress signal), `--baseline DIR` (compare), `--json`,
`--override "tool.x:path"` (swap a prompt component — used by `naming-*` to
compare STRAP vs flat naming), `--experiment NAME`.

### What happens internally

`run_test_command` → `engine::run_live` (`crates/agent/src/testing/engine.rs:51`)
**connects to the running server over `ws://<server>/ws`** and replays the
fixture's conversation through the **real chat pipeline** (runner → provider →
tools). If it can't connect it errors `"Cannot connect to Nebo … Is make dev
running?"` — proof it does not boot its own server. The agent **under test** uses
the server's configured provider (Janus/`nebo-1` by default; override with
`--model`). The resulting trace is then graded (§4) → FCSR per fixture.

Use the prebuilt `./target/debug/nebo-cli` directly rather than `cargo run` so
cargo doesn't fight `make dev` for build locks. Rebuild it deliberately
(`cargo build -p nebo-cli`) only when `make dev` is stopped.

---

## 4. The grader is hardwired to claude-code

`crates/agent/src/testing/grader.rs`:

- `grade()` (line 11) **unconditionally** calls `grade_with_claude_code()` — there
  is **no API/Janus branch**.
- `grade_with_claude_code()` (line 23) shells out to the **`claude` CLI**:
  `Command::new("claude").args(["--print","--verbose","--output-format","stream-json",
  "--dangerously-skip-permissions","--model", <grader>])`, from a neutral temp
  workspace with `CLAUDE_CODE_DISABLE_AUTO_MEMORY` + `…_CLAUDE_MDS` set (so the
  judge doesn't inhale the repo's CLAUDE.md or operator memory).

So **claude-code is always the evaluator.** `--grader <X>` only chooses which model
claude-code runs (`--model X`: `sonnet`, `opus`, `claude-sonnet-4-6`, …). It cannot
route grading anywhere else. Requires the `claude` CLI on `PATH`. The grader prompt
asks for tool-quality + model-behavior + per-assertion pass/fail + FCSR +
context-pollution, returned as strict JSON (`build_grader_prompt`).

Without `--grader`, fixtures still run and traces are saved, but there's no
LLM-judged scorecard — only the trace.

---

## 5. CRITICAL gotchas (read before running anything)

1. **Run `nebo-cli`, NOT `nebo`.** Two different crates build two different binaries:
   - `target/debug/nebo` = the **Tauri app/server** (`src-tauri`). Its `main()`
     boots a server on :27895. Running *any* subcommand of it (incl. a fixture run)
     boots a **second** server → "port in use" if `make dev` is up, or a **wedged
     full-runtime server** (gws watchers + heartbeat agents monopolize the runner;
     0 fixtures complete, ~0% CPU, no output) if it's down. **This is the trap.**
   - `target/debug/nebo-cli` = the **CLI** (`crates/cli`). `test run` connects to an
     existing server over WS; only `nebo-cli serve` boots one.
2. **`make dev` must be UP for both systems.** They drive the one running server.
   The harness is *not* "run with the server down" — that was a wrong assumption that
   wastes time. Up.
3. **Don't run the `nebo` app binary's `run`/`test run` to drive fixtures.** Same as
   #1 — it's the server, it boots a server, it wedges. The wedge looks like a hang
   (idle CPU, no output, leftover `gws` child processes); kill with
   `pkill -f "nebo (test )?run"` and reap stray `pkill -f "Nebo/nebo/plugins/gws"`.
4. **Output is block-buffered through a pipe.** `… | tail` hides per-fixture
   progress until the end. Use `--output DIR` and watch the trace files appear, or
   redirect to a file and `Read` it.
5. **claude-CLI grading is slow** (~20-40 s/fixture, cold-start each). A 17-fixture
   suite is many minutes. Background it; don't assume "no output yet" = hung — check
   the `--output` trace dir and for live `claude --print` / agent activity first.
6. **`/agent/mcp` ≠ a chat session** — no agent scope; agent-scoped writes fail there
   (§2). Use REST for workflow/agent creation.

---

## 6. Plan drift

`tests/integration/plan.md` + `TEMPLATE.md` predate several renames. The *behavior*
is correct; the plan's literal commands are stale. Confirmed 2026-06-19 (and partly
in the 2026-03-31 / 2026-06-11 runs):

| Plan says | Reality |
|---|---|
| `role` tool / `nebo/roles`, `user/roles` | renamed → `agent` / `…/agents` |
| `nebo/tools`, `nebo/workflows` dirs | gone — consolidated under `agents` |
| event `schedule: "0 9 * * 1-5"` (5-field) | param is `cron:`, **6-field** (`0 0 9 * * 1-5`) or `at: "in 1 hour"` |
| skill `catalog` action | renamed → `list` |
| keychain `store` + `label` | `add` (requires `service`); **`find` requires `label`** — inconsistent param per action (real bug) |
| message `dnd_on` / `dnd_off` | never existed; only `dnd_status` (read-only) |
| REST agent `role_md` / snake_case | `agentMd` / `agentJson` (camelCase) |
| REST workflow `definition` as object | must be a **JSON string** |
| browser `open` / `tabs` | `navigate` / `read_page` / `click` / … |
| settings `displays` | not an action (have: volume/brightness/wifi/bluetooth/battery/darkmode/sleep/lock/info/mute) |
| `ROLE-…` install codes | prefix renamed → `AGNT-…` |
| `GET /integrations/tools` | now under `/api/v1/…` |

**Open bug surfaced repeatedly:** installed (`nebo/`-namespace) skills are **not
delete-protected** — the skill tool deletes them successfully (integration plan
S-11, FAIL in both the 2026-03-31 and 2026-06-19 runs).

**`WORK-SW4Z-5XKN`** marketplace code is dead ("code not found") — needs re-issuing.

---

## 7. File / path reference map

| Thing | Path |
|---|---|
| Integration plan + results | `tests/integration/{plan,README,run-prompt}.md`, `results/` |
| Prompt fixtures | `fixtures/**/*.yaml` (repo root) |
| Suites | `suites/*.yaml` |
| Harness entry (CLI) | `crates/cli/src/main.rs` → `Commands::Test` → `run_test_command` |
| Fixture runner (WS client) | `crates/agent/src/testing/engine.rs` (`run_live`) |
| Grader (claude-code) | `crates/agent/src/testing/grader.rs` |
| Fixture/trace/report types | `crates/agent/src/testing/{fixture,trace,reporter}.rs` |
| Deterministic tool exec endpoint | `crates/server/src/handlers/mcp_server.rs` (`/agent/mcp`) |
| Built binaries | `target/{debug,release}/nebo-cli` (CLI) vs `…/nebo` (app — **do not use for tests**) |
| Data dir (macOS) | `~/Library/Application Support/Nebo/` |
| Dev log | `/tmp/nebo-dev.log` |

> Prior context that fed this doc: `[[deep-research-harness]]`, `[[feedback-first-call-success]]`,
> `[[feedback-no-mock-testing]]`, `[[feedback-iterative-ml-optimization]]`,
> `[[feedback-single-variable-optimization]]`, `[[feedback-session-isolation]]`.
