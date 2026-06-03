# Bug: 9 stale unit tests in `nebo-tools` (drifted from intentional code changes)

**Date:** June 3, 2026
**Severity:** Medium (CI/test integrity — production code was correct; the tests were stale)
**Component:** `crates/tools/` unit tests
**Status:** Fixed

## Summary

`cargo test -p nebo-tools --lib` reported **9 failures**. None were production regressions —
every failure was a test that had drifted from an intentional, already-landed code change.
The fixes update the tests to assert the *current, correct* behavior (we did **not** revert
production code to satisfy stale assertions).

| # | Test | Root cause | Fix |
|---|------|-----------|-----|
| 1 | `file_tool::tests::file_read_dedup` | Asserted a second identical read returns a `"unchanged"` cache placeholder. That path-keyed read cache was **deliberately removed** (it gaslit the model into a retry spiral — the #research read-loop incident; see `handle_read` comment in `file_tool.rs`). | Replaced with `file_read_repeat_returns_content`, asserting a repeat read returns the content again (never a placeholder). |
| 2 | `file_tool::tests::file_read_dedup_invalidation` | Name/comment referenced the removed dedup cache. The assertion (fresh content after modification) was still valid but misnamed. | Renamed to `file_read_reflects_modification` and updated the comment. |
| 3–7 | `app_tool::tests::test_{launch,quit,activate,hide,info}_missing_app` | Asserted the old literal error `"'app' parameter required"`. The tool now uses the standardized `errors::missing_param(...)` helper, which emits `"Missing required parameter 'app' for <action> action."`. | Updated the 5 assertions to the canonical wording. |
| 8 | `music_tool::tests::test_search_requires_query` | Asserted the old literal `"query is required"`; same `missing_param` standardization. | Updated the assertion to `"Missing required parameter 'query'"`. |
| 9 | `skills::loader::tests::test_user_plugin_skills_loaded` and `..._override_marketplace_plugin_skills` | `unwrap()` panic — the embedded skill was never loaded. The loader gained an `is_plugin_active(slug)` gate (only load skills for **ready** plugins — see `skill-loader-scans-uninstalled-plugins.md`), which calls `PluginStore::is_ready` → `get_manifest`. The test fixture `create_plugin_embedded_skill` wrote `skills/<name>/SKILL.md` but **no `plugin.json`**, so `is_ready` returned false and the skill was skipped. | Updated the fixture to also write a minimal `plugin.json` at `<slug>/<version>/`, so the plugin is "ready" — matching real install conditions. |

(#9 covers two tests, giving 9 failing tests total.)

## Why these are fixes, not workarounds

Per `docs/sme/CODE_AUDITOR.md` Rule 10 (no quick fixes): in every case the **production
code is the intended behavior** (no suppression cache, standardized error messages, skills
only load for ready plugins). The tests had simply not been updated when those changes
landed. Reverting the code to pass the old assertions would re-introduce the very bugs the
changes fixed. So the tests were brought in line with current behavior.

## Verification

`cargo test -p nebo-tools --lib` → **190 passed, 0 failed** (was 181 passed / 9 failed).

---

## Addendum: 3 further failures surfaced in `nebo-agent`

Running `cargo test -p nebo-agent` surfaced 3 more failures (a different crate, not in the
original 9). Two are the same stale-test class; the third is a **real production bug**.

| Test | Root cause | Fix |
|------|-----------|-----|
| `tool_filter::tests::test_core_tools_always_included` | Asserted `web` is filtered out without keywords. `web`/`os` were made **core/always-on** in the prior session ("web is not optional"). | Updated assertions: `web`/`os` are core (always included); kept `loop` as the filtered (non-core) case. |
| `tool_filter::tests::test_contextual_keyword_activates_context` | Same — asserted `web` filtered out, but it's now always-on. | Swapped the negative case to `loop` (a genuinely non-core tool) so keyword-gating is still exercised. |
| `advisors::loader::tests::test_list_enabled` | **Real bug** — `db::Store::new(":memory:")` panicked: `UNIQUE constraint failed: _nebo_migrations.version` while recording `0099`. | See below. |

### Real bug: duplicate migration version `0099` (broke every fresh DB init)

Two migration files on `main` shared version `0099`:
- `0099_plugin_account_profiles.sql` (committed **2026-05-28**, idempotent — `CREATE TABLE IF NOT EXISTS`)
- `0099_agent_handle_color.sql` (committed **2026-05-29**, the later duplicate — non-idempotent `ALTER TABLE ADD COLUMN`)

`crates/db/src/migrate.rs` records the numeric filename prefix as `version INTEGER PRIMARY KEY`,
and snapshots the applied set **once** before the apply loop. On a fresh DB both files attempt
`INSERT … version 99` → the second hits the PRIMARY KEY/UNIQUE constraint → `migrate()` returns
`Err` → **the app fails to start on every fresh install after 2026-05-29.** (The advisors test
hit this because it builds a fresh `:memory:` DB.)

**Fix:** renamed `0099_plugin_account_profiles.sql` → `0102_plugin_account_profiles.sql`
(0101 was the previous max). Chosen because it is **idempotent**, so re-applying it on any
already-migrated DB is safe (zero crash risk), it fixes fresh installs and DBs that hit the
collision, and it causes **no regression** to any existing population. The non-idempotent
`agent_handle_color` was left at `0099` precisely because renumbering an `ADD COLUMN` migration
could hard-crash a DB that already applied it.

**Caveat for existing internal DBs (flagged, not auto-handled):** a DB that ran migrations
during the 2026-05-28 → 05-29 collision window may already have `0099` recorded for whichever
file sorted first, leaving the other migration permanently skipped (e.g. `agents.handle`/`color`
columns missing — these are nullable with documented graceful fallback). This is the pre-existing
state on `main`, not a regression from this rename. If such DBs need the missing columns, a
follow-up *idempotent* migration (or a dev-DB reset) is the clean path — decide per environment.

### Verification (agent crate)

`cargo test -p nebo-agent` → **276 passed, 0 failed**.
