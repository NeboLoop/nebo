# Plan: Remove Standalone WORK Code Type

## Context

Workflows are now integrated into agents as inline bindings (defined in `agent.json` under `workflows: {}`). The comment at `deps.rs:476` confirms: "Workflows are now inline — only skill dependencies are extracted." The `extract_agent_deps()` function no longer produces `DepType::Workflow`.

The standalone `WORK-XXXX-XXXX` code path is dead weight. No frontend UI for standalone workflow installation, and the preferred path is `AGNT-` codes with embedded workflow bindings. The re-install logic we just added to `handle_work_code()` is unnecessary.

## Dependencies Still Active (validation findings)

Before removing, be aware these paths are still wired:

- **Store product install** (`handlers/store.rs`) routes through `codes.rs` — marketplace "workflow" products use `WORK-XXXX-XXXX` codes for installation
- **Frontend installed page** explicitly fetches `type=workflow` products and displays them
- **Scheduler** still has a `"workflow"` task type for standalone scheduled workflows
- **Public docs** (`docs/publishers-guide/workflows.md`) document WORK code support

## What stays (still used by agent-bound workflows)

- Workflow HTTP handlers (`/workflows/*`) — still registered, used for agent workflow management
- WorkflowManager trait (list, resolve, run, status, cancel) — runtime workflow execution
- `extract_workflow_deps()` and `DepType::Workflow` in deps.rs — called from POST /workflows handler
- Scheduler support for `agent_workflow` task type
- DB schema (`workflows`, `workflow_runs`, `agent_workflows` tables)

## Changes

### 1. `crates/server/src/codes.rs`

- Remove `Work` from `CodeType` enum
- Remove `"WORK-"` detection in `detect_code()` (lines 47-48)
- Remove `CodeType::Work` arms from both dispatch sites (handle_code + submit_code)
- Remove `handle_work_code()` function entirely (~lines 246-312)
- Remove `persist_workflow_artifact()` function entirely (~lines 684-778)
- Remove WORK test case (line 1139)

### 2. `crates/tools/src/workflows/work_tool.rs`

- Remove `install` action — return error "workflows are now installed via agent codes (AGNT-XXXX-XXXX)"
- Remove `uninstall` action — same error message
- Remove `code` field from `WorkInput` struct
- Update doc comment

### 3. `crates/tools/src/workflows/manager.rs` (trait)

- Remove `install()` method from trait
- Remove `uninstall()` method from trait

### 4. `crates/server/src/workflow_manager.rs` (impl)

- Remove `install()` impl
- Remove `uninstall()` impl

### 5. `crates/comm/src/api.rs`

- Remove `install_workflow()` — only callers are codes.rs (removed) and workflow_manager.rs (removed)
- Remove `uninstall_workflow()` — zero callers in entire codebase
- Keep `list_workflows()` — may be used by frontend store

### Not changed

- `crates/server/src/deps.rs` — `DepType::Workflow`, `extract_workflow_deps()`, `is_workflow_installed()`, `install_workflow()` all stay. The Workflow variant becomes dead code in the cascade resolver (no code produces it anymore), but it's still referenced from `handlers/workflows.rs:81` and removing it risks breaking the POST /workflows endpoint. Harmless to keep.
- `crates/server/src/handlers/workflows.rs` — untouched, preserves existing functionality
- Scheduler — untouched, preserves existing scheduled workflows

## Files to modify

| File | Change |
|------|--------|
| `crates/server/src/codes.rs` | Remove Work variant, handler, persistence fn |
| `crates/tools/src/workflows/work_tool.rs` | Remove install/uninstall actions |
| `crates/tools/src/workflows/manager.rs` | Remove install/uninstall from trait |
| `crates/server/src/workflow_manager.rs` | Remove install/uninstall impls |
| `crates/comm/src/api.rs` | Remove install_workflow, uninstall_workflow |

## Verification

1. `cargo build --release -p nebo-cli` — compiles clean
2. `cargo test -p nebo-server` — passes (WORK test removed)
3. `AGNT-` codes still install agents with embedded workflows
4. `SKIL-` and `PLUG-` codes still work
5. `WORK-XXXX-XXXX` paste → not recognized as a code, passes through to agent as normal chat
6. `work(action: "list/run/status/cancel")` still works
7. Existing workflows in DB still execute via scheduler
