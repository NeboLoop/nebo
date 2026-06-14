# Tech Debt Register

Running list of known tech debt: stale code, competing pathways, dead guards, and
shortcuts that should be repaid. One entry per item. Keep entries short; link to the SME
doc or plan that covers the fix. When an item is fixed, move it to **Resolved** with the
commit SHA.

Conventions:
- **ID:** `TD-NNN`, monotonic, never reused.
- **Severity:** `security` > `correctness` > `maintainability`.
- **Status:** `open` · `in-progress` · `resolved`.

---

## Open

### TD-003 — Two competing chat-title generators race on every run
- **Severity:** maintainability · **Status:** open · **Found:** 2026-06-13
- **Where:** `crates/agent/src/runner.rs:880` and `crates/server/src/chat_dispatch.rs:1141`
  (`generate_chat_title_if_needed`)
- **What:** Both spawn a background title generator after a run completes, both check the same
  "is the title still a placeholder" condition, and both call `update_chat_title`. The runner
  one stores silently (no `chat_title_updated` broadcast, no loop push); the dispatch one
  stores **and** broadcasts + pushes to the loop (commit `c0e03144`). For a `run_chat` run both
  fire and race; whichever wins, the other no-ops.
- **Correction (not a simple delete):** `runner.run` is called from 5 paths — `chat_dispatch`
  (run_chat), `scheduler`, `handlers/voice`, `handlers/mcp_server`, `orchestrator`. The
  runner-side generator is the ONLY titler for the 4 non-dispatch paths; deleting it would
  drop their auto-titling. The race is exclusive to `run_chat`.
- **Fix:** add `skip_title_gen: bool` to `RunRequest` (default false). `run_chat` sets it
  `true` (it titles + broadcasts + loop-pushes itself); the runner-side generator guards on
  `!req.skip_title_gen`. No coverage loss, race eliminated. (Loop-push must stay dispatch-side
  — the runner crate can't reach the server's ClientHub / `codes::push_chat_title_to_loop`.)

### TD-004 — `discover` and `discover_summaries` are near-duplicate functions
- **Severity:** maintainability · **Status:** open · **Found:** 2026-06-13
- **Where:** `crates/tools/src/skills/loader.rs` (`discover` returns `Vec<Skill>`,
  `discover_summaries` returns `Vec<SkillSummary>`)
- **What:** Two ~40-line functions with identical tokenization, field-matching, and scoring
  logic. The hyphenated-name bug (fixed in `452ef881`) existed in BOTH and had to be patched
  twice — exactly the failure mode duplicated logic invites.
- **Fix:** extract the match/score core into one helper that both thin wrappers call (one maps
  to `Skill`, the other to `SkillSummary`), so matching logic lives in one place.

---

## Resolved

### TD-001 — Origin deny-list keys stale after `system → os` rename
- **Severity:** security · **Resolved:** 2026-06-13 (owner-full-access change)
- Deny key changed `"shell"`/`"system:shell"` → `"os:shell"` in `default_origin_deny_list`
  (`policy.rs`); `test_origin_deny` rewritten to use the real `"os"` tool name (it previously
  passed against pre-rename names, masking the break). Shell-deny for `Comm`/`App`/`Skill` now
  actually fires. Landed together with owner-full-access so the owner (`is_personal` →
  `Origin::User`) keeps full shell while third-party comm is restricted.

### TD-002 — `check_path_scope` didn't match the renamed `os` tool
- **Severity:** security · **Resolved:** 2026-06-13 (owner-full-access change)
- `check_path_scope` now matches `"os"` and dispatches by `resource` (`file`/`shell`) to the
  existing scope checks; `test_os_tool_path_scope_enforced` added. `allowed_paths` restrictions
  are enforced for `os(file/shell)` again.
