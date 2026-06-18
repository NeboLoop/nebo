# Design: Conversation Frames — one canonical run lifecycle

**Status:** proposed (design review before code)
**Date:** 2026-06-18
**Author trail:** prompted by a live "death spiral" — an agent hit a persistent Drive 401 and retried auth→browser→curl→gws unboundedly (≈15+ tool calls, never stopped, user had to abort). Root cause: Nebo has **no terminal failure state** for a chat run; every tool error is fed back to the model, which improvises forever.

> **The fix is not a circuit breaker.** A failure *counter* is a band-aid (CODE_AUDITOR Rule 10). The right model — Claude's — is **error classification + explicit lifecycle states**, where an *unrecoverable* error moves the run to a terminal `failed` state immediately (on first occurrence, by class), and control returns to the user.

---

## 1. The pattern we're adopting (Claude's frames)

A **frame** is one unit of agent work with an explicit lifecycle:

```
pending ──▶ processing ──▶ completed
                       ├──▶ cancelled   (user/system aborted)
                       └──▶ failed      (unrecoverable error)
                                │
            completed ─────────▶ compacted   (messages summarized, long convos)
```

Salient properties (from the Claude desktop reference, `~/claude-source/docs/sme/05-conversation-frames.md`):

- **Terminal states.** `failed` = "agent encountered an unrecoverable error." The loop stops; it does not keep iterating.
- **One lifecycle for everything.** Root conversations *and* delegated sub-agents *and* (for us) workflow runs are all frames, distinguished by `agent_name` / `parent_frame_id`.
- **Frame tree.** Delegation creates a child frame (`parent_frame_id` → coordinator). The parent `wait_for_notification`s the child.
- **Resumption.** A resumed frame marks `_resumed_from_cancelled/_failed`; orphaned `tool_use` blocks get synthetic `tool_result`s (`[Interrupted] …`) so the message history stays valid.
- **Per-frame accounting.** `input_tokens` / `output_tokens` / `total_cost` accumulate per frame.

What makes the death spiral impossible here is **classification**, not counting: an auth/permission/connection error is *terminal-for-the-agent* → `failed` now, surfaced to the user, with the reconnect affordance — the agent never improvises around it or re-authenticates itself.

---

## 2. The Rule 8 contract (the centerpiece)

**CODE_AUDITOR §8.1:** *"if functionality exists in one place, it must NOT be duplicated in another. Every capability has ONE canonical implementation."*

Nebo already has **four** run-lifecycle-ish models. The single biggest risk of "adopting frames" is shipping a **fifth** that runs alongside them. That is forbidden. Frames is a **consolidation**: it becomes the one canonical lifecycle and **replaces + deletes** the others. Per [[no-legacy-support]] (45 beta users, hard cutovers) the replaced pathways are deleted outright — no dual-running (also satisfies Rule 3, no dead code).

### Pathways this REPLACES (and deletes)

| Existing pathway | Where | Disposition under frames |
|---|---|---|
| `workflow_runs` lifecycle (status: running/completed/**failed**/cancelled, error, started/completed_at) | `crates/db` + `handlers/workflows.rs` | **Becomes the template.** Generalize this table/state-machine into the frame lifecycle — it is already frame-shaped. |
| Chat-run tracking (`register_run` / `run_handle`) | `crates/server/src/chat_dispatch.rs` | **Folded in.** A chat turn = a root frame. `register_run` creates/transitions a frame instead of its own ad-hoc handle. |
| Sub-agent delegation state (`subagent:<parent>:<child>` session keys) | `crates/agent/src/orchestrator.rs` | **Folded in** as the frame *tree* (`parent_frame_id`). Delegation creates a child frame. (Also fixes the nested-session-key id-resolution bug we patched in `plugin_tool.rs`.) |
| Live run counters (`iteration_count`, token totals on `Runner`) | `crates/agent/src/runner.rs` | **Folded in** as per-frame accounting. |

### Pathways this does NOT touch (different concerns — keep separate)

| Pathway | Why it stays |
|---|---|
| `task_graph` / `TaskStatus` | The agent's *to-do list* within a run, not the run's lifecycle. Forcing it into frames would itself be a Rule-8 conflation. |
| `sessions` / `chats` | Storage of conversation *content* (message history, active_chat_id). A frame *references* a chat; it doesn't replace it. |

**A `FRAMES.md` reviewer's job:** confirm every run-lifecycle responsibility maps to exactly one of the rows above, and the "replaces" rows have a concrete deletion in the migration plan.

---

## 3. Target model (sketch — to be refined in review)

- **Table `frames`** (generalize `workflow_runs`): `id`, `parent_frame_id` (nullable), `agent_id`, `kind` (`chat` | `workflow` | `subagent`), `status` (`pending`/`processing`/`completed`/`cancelled`/`failed`/`compacted`), `status_description`, `error` (nullable, set on `failed`), `chat_id` (nullable ref), `input_tokens`, `output_tokens`, `total_cost`, `created_at`/`completed_at`.
- **One transition API** (in `agent`/`server`, handler-owns-logic per §8.3 — no new service layer): `create_frame`, `set_processing`, `complete`, `cancel`, `fail(error)`. `fail` is the terminal path.
- **Error classification** (the death-spiral fix): a single classifier maps a tool error to `Terminal { user_message }` vs `Retryable`. **Terminal → `fail()` + end the loop + surface.** Auth / permission / connection / missing-account errors are Terminal. This *extends the existing* `is_auth_error` path in `plugin_tool.rs` — it does NOT add a parallel mechanism (§8.1, Rule 10).
- **Cancellation** reuses the existing `CancellationToken` (Rule 9), not a new flag.
- **Frontend**: frame state surfaced via the generated client (`make gen`), never hand-rolled (Rule 5).

---

## 4. Migration phases

**Phase 1 — terminal-error slice (SHIPPED; first brick of frames).**

> **Why "ask permission" is the wrong model here, and "stop" is right.** Most of
> what Nebo does runs **autonomously in workflows** — there is no human to pause
> for, and permission is granted up-front at agent/workflow setup. So Claude
> Code's interactive `ask` permission flow does not apply to the autonomous
> majority. A workflow that hits a dead account **cannot ask** and **must not
> spiral** (no one to hit stop). The correct behavior is therefore: **deny the
> futile action, classify the error as terminal, and stop the run** — surfaced in
> chat, recorded + notified in a workflow.

What landed (`ToolResult.terminal` + runner + plugin_tool):
1. **`ToolResult::terminal(content)`** — a terminal (unrecoverable) error flag. Set ONLY here (narrow): healthy long-running tasks never trip it, so this is classification, not a counter, and does not kill legitimate multi-step work.
2. **Agent self-auth is denied.** `auth login`/`logout`/`setup` are refused at the plugin-dispatch point with a terminal "reconnect in Connected Accounts" message — deny, not ask (futile in a workflow; the user's out-of-band action in chat). This is the spiral's fuel, removed. Read-only `auth status`/`export` stay allowed.
3. **Auth-expired + system reauth failed → terminal.** Surfaced as "reconnect this account," not retried.
4. **Runner stops the run on a terminal result** — emit the message, break the agentic loop (mirrors the existing circuit-breaker's emit-then-break). No human needed → safe for autonomous workflows.
5. **Front-end prevention (prompt steering).** A `prompt.rs` bullet tells the agent up front it cannot sign in/re-authenticate plugins itself — so it never starts down the auth path. The dispatch-side deny (item 2) becomes a rarely-hit backstop instead of the only thing stopping the spiral. (Live harness evidence: `fixtures/tools/plugin-auth-no-self-reauth.yaml` went 2/5 → **5/5**, 4.0 → **0.0** tool calls, pollution 0.87 → **0.00**, once the bullet was added — the deny alone left the model spiraling *up to* the deny.)

Deferred to Phase 2/3 (frames proper):
- **Granular multi-part handling** — one sub-capability failing shouldn't fail a whole multi-step run. Needs the frame *tree* (a child frame fails; the parent decides). Phase 1 stops the common single-purpose run cleanly.
- **`run_auth_login` auto-launch → HIL/notify** — for chat, surface a reconnect prompt; for workflows, fail + notify rather than auto-spawning an OAuth flow no one completes.
- The real `failed` frame **state** + async workflow notification.

**Phase 2 — promote `workflow_runs` → `frames`.** Generalize the table + transition API; route chat runs through it (root frames). Delete `register_run`'s ad-hoc handle.

**Phase 3 — frame tree.** Fold orchestrator delegation into `parent_frame_id`; unify resumption (synthetic tool_results for orphaned tool_use). Delete the now-redundant subagent-key state.

Each phase ends with the replaced pathway **deleted**, verified against Rule 3.

---

## 5. Open questions (for review)

- Generalize `workflow_runs` in place vs. a new `frames` table the workflow engine migrates onto? (In-place avoids a transient competing pathway but touches the workflow engine; a new table is cleaner but must delete `workflow_runs` in the same change.)
- Does `compacted` map to Nebo's existing compaction, or stay out of scope for v1?
- Exact terminal-error taxonomy: which plugin/tool error classes are Terminal vs Retryable (auth, permission, quota/429, network, validation)? Validation errors (the `unrecognized subcommand` that started the spiral) are **not** terminal — they're a *surfacing* problem (the agent should discover the right command), so they must NOT be misclassified as auth (the agent's own mistake that triggered this whole investigation).

---

## 6. Why this is the right fix (not a circuit breaker)

A counter ("stop after N failures") treats all failures alike and fires late, after waste. Classification fires **immediately** on the *kind* of error that can't be retried, returns control to the user with an actionable message, and — as a frame state — composes with delegation, resumption, and accounting. It's the difference between a fuse and a diagnosis.
