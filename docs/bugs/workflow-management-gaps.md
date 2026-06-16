# Workflow Management Gaps Report

**Date:** June 14, 2026  
**Prepared by:** Alma's Assistant  
**Subject:** Identified gaps preventing proper workflow management for agents

---

## Executive Summary

This report documents the limitations in the current workflow management toolset that prevent comprehensive administration of agent workflows. While basic operations are available, critical configuration and inspection capabilities are missing.

---

## Current Capabilities

### Available Operations

| Operation | Status | Notes |
|-----------|--------|-------|
| List installed workflows | ✅ | Returns workflow names, status, and activity counts |
| Run workflows | ✅ | Supports optional input parameters |
| Check run history | ✅ | Shows execution count and recent runs |
| Toggle workflows | ✅ | Enable/disable functionality |
| Uninstall workflows | ✅ | Requires workflow ID |

### Example: Installed Workflows

```
1. Hourly Alma Channel Report - Enabled (6 activities)
2. desktop-cleanup - Enabled (0 activities)
```

Both workflows show zero executions despite being enabled.

---

## Critical Gaps

### 1. **Cannot View Workflow Definitions**

**Impact:** Unable to understand what a workflow does without external documentation.

**Missing:**
- Read workflow manifest files (agent.json)
- Inspect activity definitions (per-step config, sequence)
- See dependency graphs between workflows and agents

**Current Behavior:** The `list` response serializes the full `WorkflowInfo` struct (`crates/tools/src/workflows/manager.rs:9-17`): `id`, `name`, `version`, `description`, `isEnabled`, `triggerCount`, and `activityCount`. So basic identity and counts — including **trigger count** — *are* visible. What's missing is the manifest body itself: the activity definitions and trigger expressions, not their counts.

### 2. **Cannot Configure Triggers**

**Impact:** Cannot set up or modify when/why workflows execute.

**Missing:**
- Create new triggers
- Edit existing trigger schedules
- View trigger expressions
- Test trigger conditions

**Example Gap:** The "Hourly Alma Channel Report" has no recorded runs. The `list` response exposes `triggerCount`, and for this workflow it is **0** — so we *can* determine the immediate cause: **no triggers are configured**. What we still cannot determine, without manifest access, is *why* the trigger is missing or what it should be:
- Whether the trigger was never installed vs. dropped during setup
- What schedule/expression it was supposed to carry
- Whether a misconfigured (but present) trigger would have fired

The diagnostic blindness is about trigger *content*, not trigger *existence* — `triggerCount` already answers the existence question.

### 3. **Limited Activity Inspection**

**Impact:** Cannot diagnose why workflows aren't running or failing.

**Missing (per-activity granularity):**
- Detailed activity logs per run
- Error messages attributed to a *specific* failed activity
- Per-step timing data (which activity was slow)
- Input/output values for each activity

**Available at the run level:** `WorkflowRunInfo` (`crates/tools/src/workflows/manager.rs:20-31`) exposes `status`, `error`, `started_at`, `completed_at`, `total_tokens_used`, and `trigger_type`. So run-level **error text**, **start/end timing (and thus duration)**, and **token usage** *are* surfaced via `runs`/`status` — this is not binary success/failure.

**Current State:** Run-level outcomes (status, error, timing, tokens) are exposed; what's missing is **per-activity** breakdown — errors and timing are attributed to the run as a whole, not to the individual step that produced them.

### 4. **No Manifest Editing**

**Impact:** Cannot fix broken workflows or update configurations.

**Missing:**
- Write workflow manifests
- Update trigger definitions
- Modify activity sequences
- Change dependencies

### 5. **Agent Configuration Blindness**

**Impact:** Cannot verify if underlying agent configs match workflow expectations.

**Missing:**
- View agent.json files referenced by workflows
- Check skill dependencies
- Verify plugin installations
- Validate resource permissions

---

## Technical Root Causes

### Tool Limitations

The `work` resource provides only lifecycle and dispatch operations:

```json
{
  "action": ["list", "create", "install", "uninstall", "run", "status", "runs", "toggle", "cancel"]
}
```

> Note: `cancel` (`work(action: "cancel", id: "<run-id>")`, `crates/tools/src/workflows/work_tool.rs:96-104`) exists in the dispatch code but is omitted from the tool's own description string.

No actions exist for:
- `get-definition`
- `update-trigger`
- `view-manifest`
- `edit-config`

### File System Access

Workflows likely store configuration in a specific directory (possibly under `/Users/almatuck/Library/Application Support/Nebo/files` or similar), but there's no documented path and no direct file access mechanism exposed through the workflow tools.

### MCP Server Integration

While several MCP servers are connected (neboai, monument, janus, payrollhub), none appear to expose full workflow CRUD operations. The `mcp__neboai__agent` tool manages agent personas but not their associated workflows.

---

## Affected Workflows

### Hourly Alma Channel Report

- **Status:** Enabled
- **Activities:** 6
- **Runs:** 0
- **Triggers:** 0 (per `triggerCount` in the `list` response)
- **Issue:** No triggers configured — directly explains the 0 runs.

**Confirmed:** `triggerCount: 0`, so the workflow has nothing wired to fire it. This is the root cause of the missing runs.

**Cannot Verify (requires manifest access):** *why* the trigger is absent and what it should have been — whether it was never installed, dropped during setup, or what schedule/expression it was meant to carry.

### desktop-cleanup

- **Status:** Enabled
- **Activities:** 0
- **Runs:** 0
- **Issue:** Empty workflow (no activities defined)

**Likely Cause:** Incomplete setup or deleted activities.

---

## Recommendations

### Short-Term Mitigations

1. **Manual File Access**
   - Document the workflow configuration directory path
   - Grant read/write access to manifest files
   - Use `os(resource: "file", action: "read")` for inspection

2. **External Documentation**
   - Maintain separate docs for workflow purposes and configurations
   - Track trigger schedules in a shared spreadsheet
   - Log run attempts and failures manually

3. **Tool Discovery**
   - Run `tool_search(query: "workflow")` to find additional capabilities
   - Check `plugin(action: "list")` for workflow-related plugins
   - Explore `skill(action: "discover", query: "workflow management")`

### Long-Term Solutions

1. **Enhanced Workflow Tools**
   - Add `get`, `update`, `delete` actions to the `work` resource
   - Expose manifest file paths in the `list` response
   - Provide detailed run logs via `runs` with pagination

2. **MCP Server Expansion**
   - Build a dedicated workflow management MCP server
   - Expose full CRUD operations on workflow definitions
   - Integrate with the Janus gateway for centralized management

3. **File System Integration**
   - Standardize workflow storage location
   - Document the schema for workflow manifests
   - Enable direct file editing as a fallback mechanism

---

## Investigation Steps Taken

1. ✅ Listed all installed workflows
2. ✅ Checked run history for both workflows
3. ✅ Attempted to retrieve workflow definitions via MCP tools
4. ❌ Could not access workflow manifest files (no path known)
5. ❌ Could not inspect trigger configurations
6. ❌ Could not view activity-level details

---

## Next Actions

To proceed with fixing these gaps, I recommend:

1. **Discover workflow storage location**
   ```bash
   # Search for workflow manifest files
   os(resource: "shell", action: "exec", command: "find ~/Library/Application\\ Support/Nebo -name \"agent.json\" 2>/dev/null | head -20")
   ```

2. **Check for additional tools**
   ```
   tool_search(query: "workflow manifest")
   tool_search(query: "agent configuration")
   ```

3. **Review installed plugins**
   ```
   plugin(action: "list")
   ```

Would you like me to execute any of these investigation steps?

---

## Appendix: Tool Reference

### Current Workflow Tool Schema

```
work(action: "list")                    # List all workflows
work(action: "create", ...)             # Create new workflow
work(action: "install", code: "...")    # Install from marketplace
work(action: "uninstall", id: "...")    # Remove workflow
work(resource: "<name>", action: "run") # Execute workflow
work(resource: "<name>", action: "status") # Check latest run
work(resource: "<name>", action: "runs")   # List run history
work(resource: "<name>", action: "toggle") # Enable/disable
work(action: "cancel", id: "<run-id>")      # Cancel a running execution
```

### Missing Operations

- `work(resource: "<name>", action: "get")` - Get full definition
- `work(resource: "<name>", action: "update")` - Update configuration
- `work(resource: "<name>", action: "triggers")` - List triggers
- `work(resource: "<name>", action: "activities")` - List activities
- `work(resource: "<name>", action: "logs")` - View execution logs

---

*Report generated automatically by Alma's Assistant*
