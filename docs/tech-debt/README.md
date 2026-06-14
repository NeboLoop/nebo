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

_No open items._

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

### TD-003 — Two competing chat-title generators raced on every run
- **Severity:** maintainability · **Resolved:** 2026-06-13
- Added `skip_title_gen: bool` to `RunRequest`; `run_chat` sets it `true` (it titles +
  broadcasts + loop-pushes itself) and the runner-side generator now guards on
  `!skip_memory && !skip_title_gen`. The 4 non-dispatch run paths (scheduler, voice,
  mcp_server, orchestrator) keep the runner-side titler — no coverage loss, race gone.

### TD-004 — `discover` and `discover_summaries` were near-duplicate functions
- **Severity:** maintainability · **Resolved:** 2026-06-13
- Extracted the shared tokenize/match/score logic into one private `discover_scored` helper
  returning sorted `Vec<Skill>`; `discover` returns it directly and `discover_summaries` maps
  to `SkillSummary`. Matching logic (incl. the hyphen normalization) now lives in one place.
