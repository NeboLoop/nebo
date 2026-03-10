# Bug: PPTX Skill Execution Failure

**Date:** 2026-03-10
**Severity:** High
**Component:** `crates/napp/` — skill execution / `nebo-office` binary

## Summary

The `pptx` skill is installed and its documentation loads correctly via `skill(action: "help", name: "pptx")`, but execution fails. The skill requires the `nebo-office` binary to generate .pptx files, and the execution path does not work.

## Steps to Reproduce

1. Verify skill is installed: `skill(action: "catalog")` — pptx shows as `[nebo]`
2. Load skill docs: `skill(action: "help", name: "pptx")` — succeeds, full spec returned
3. Attempt to execute: `execute(skill: "pptx", script: "scripts/create.py", args: {...})` or equivalent

## Expected Behavior

The skill should either:
- Execute `nebo-office pptx create spec.json -o output.pptx` via the bundled binary
- OR provide a clear execution path documented in the skill

## Actual Behavior

The skill has no clear execution entry point. The SKILL.md documents CLI commands (`nebo-office pptx create ...`) but:
1. **No `nebo-office` binary found** on system PATH or in skill package
2. **No scripts/ directory** in the skill package to call via `execute()`
3. The skill is essentially documentation-only — it describes the JSON spec format but provides no executable

## Investigation Needed

1. Where is `nebo-office` built/distributed? Is it a separate binary that needs to be compiled from `nebo-rs`?
2. Should the skill package include the binary, or should it reference a system-installed binary?
3. Is there an `execute()` path that works for sealed `.napp` skills with compiled binaries?

## Workaround

None currently. Cannot generate .pptx files through the skill system.

## Related

- Skill execution system: `crates/tools/src/execute_tool.rs`
- Sealed .napp reader: `crates/napp/src/reader.rs`
- Runtime provisioning: `/tmp/nebo-runtimes/`
