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

### TD-001 — Origin deny-list keys are stale after the `system → os` rename
- **Severity:** security · **Status:** open · **Found:** 2026-06-13
- **Where:** `crates/tools/src/policy.rs` (`default_origin_deny_list`, `is_denied_for_origin`)
- **What:** The per-origin deny-list keys the `shell` tool as `"shell"` / `"system:shell"`,
  but after the tool rename the live tool name is `"os"` and the call site
  (`registry.rs:495`) passes `name = "os"`, `resource = Some("shell")`. `is_denied_for_origin`
  checks `denied.contains("os")` and `denied.contains("os:shell")` — neither matches the
  stored keys. **Result: the intended shell-deny for `Origin::Comm` / `App` / `Skill` never
  fires.** A remote/comm sender (or app, or skill) can currently invoke `os(resource:"shell")`
  on the owner's machine despite the deny-list.
- **Why it hid:** the unit test `test_origin_deny` (`policy.rs:324`) calls
  `is_denied_for_origin(Origin::Comm, "shell", None)` and `(Origin::App, "system", Some("shell"))`
  — i.e. the **pre-rename** names — so it stays green while production is broken. False
  confidence.
- **Fix:** update the deny keys to the `os` namespace (`"os:shell"`, and bare `"os"` is too
  broad — must be the compound), and rewrite the test to call with the real registered tool
  name (`"os"`, `Some("shell")`). Audit for any other origin-keyed entries that assume old
  names.
- **Related:** blocks the "third-party stays restricted" half of
  `docs/plans/owner-full-access-from-comm.md`.

### TD-002 — `check_path_scope` does not match the renamed `os` tool
- **Severity:** security · **Status:** open · **Found:** 2026-06-13
- **Where:** `crates/tools/src/safeguard.rs` (`check_path_scope`)
- **What:** The path-scope guard matches `tool_name` against `"system" | "file" | "shell"`,
  but the registry calls it with `name = "os"` (`registry.rs:486`). So `os` falls through to
  `_ => None` and **file/shell path scoping is silently disabled for the `os` tool** whenever
  `allowed_paths` is set. `allowed_paths` restrictions are not enforced for any `os(file,…)`
  or `os(shell,…)` call.
- **Fix:** match `"os"` and dispatch by `resource` (`file`/`shell`) instead of by the old
  per-tool names. Add a test that an out-of-scope `os(file, write)` is blocked when
  `allowed_paths` is non-empty.

### TD-003 — Two competing chat-title generators race on every run
- **Severity:** maintainability · **Status:** open · **Found:** 2026-06-13
- **Where:** `crates/agent/src/runner.rs:880` and `crates/server/src/chat_dispatch.rs:1141`
  (`generate_chat_title_if_needed`)
- **What:** Both spawn a background title generator after a run completes, both check the same
  "is the title still a placeholder" condition, and both call `update_chat_title`. The runner
  one stores silently (no `chat_title_updated` broadcast, no loop push); the dispatch one
  stores **and** broadcasts (and now pushes to the loop — see commit `c0e03144`). They race;
  whichever wins, the other no-ops. This is a competing pathway (CLAUDE.md Rule 8): two ways
  to do one thing.
- **Fix:** delete the runner-side generator (`runner.rs:880-914`) and keep the
  dispatch-side one as the single canonical title finalizer (it broadcasts + propagates to
  the loop). Verify no non-dispatch run path relied on the runner-side generator for titles.

---

## Resolved

_(none yet)_
