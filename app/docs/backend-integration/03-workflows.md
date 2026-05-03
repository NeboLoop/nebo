# Workflow System — Backend Integration Guide

Complete documentation of the frontend workflow system: data models, API surface, mutation semantics, and component architecture. Everything the backend needs to serve.

---

## 1. Core Data Models

### 1.1 Workflow

A workflow belongs to an agent. Each agent can have multiple named workflows stored as `Record<string, Workflow>`.

```typescript
interface Workflow {
  trigger: WorkflowTrigger;
  description: string;
  isActive: boolean;
  activities: Activity[];
  connections?: WorkflowConnection[];  // If absent, linear chain is inferred from array order
  emit?: string;                       // Event name emitted on completion (e.g. "brief.morning.delivered")
  schedule?: string;                   // Legacy field, redundant with trigger.schedule
  lastFired?: string;                  // Human-readable timestamp (e.g. "Today, 8:00 AM")
  source?: 'marketplace';              // If installed from marketplace (read-only)
}
```

### 1.2 WorkflowTrigger

```typescript
interface WorkflowTrigger {
  type: 'schedule' | 'heartbeat' | 'event' | 'manual';
  schedule?: string;     // Only for type=schedule. Human-readable: "8:00 AM daily", "3:00 PM weekdays", "Monday 9:00 AM"
  event?: string;        // Only for type=event. Comma-separated: "GitHub PR opened", "email.received"
  interval?: string;     // Only for type=heartbeat. Options: "5m","10m","15m","30m","1h","2h","4h","8h","24h"
  window?: {             // Only for type=heartbeat. Optional operating hours restriction
    start: string;       // 24h format: "09:00"
    end: string;         // 24h format: "18:00"
  };
}
```

**Schedule string format** — parsed by `parseScheduleString()`:
- `"8:00 AM daily"` → every day at 8:00 AM
- `"3:00 PM weekdays"` → Mon-Fri at 3:00 PM
- `"9:00 AM weekends"` → Sat-Sun at 9:00 AM
- `"Monday 9:00 AM"` → Monday only at 9:00 AM
- `"9:00 AM Mon, Wed, Fri"` → custom days

### 1.3 Activity

```typescript
interface Activity {
  id: string;                          // Unique within workflow. Kebab-case: "scan-sources", "check-urgency"
  intent: string;                      // What this step does: "Check all configured topic sources"
  type?: ActivityType;                 // One of 12 types (defaults to 'custom' if absent)
  skills?: string[];                   // Skill URIs: ["@nebo/skills/web-scraper@^1.0.0"]
  steps?: string[];                    // Human-readable steps: ["Search news", "Extract findings"]
  params?: Record<string, any>;        // Type-specific parameter values (see §2)
}
```

### 1.4 WorkflowConnection

Defines the DAG structure. If `connections` is absent from a workflow, the frontend infers a linear chain: `trigger → activity[0] → activity[1] → ... → emit`.

```typescript
interface WorkflowConnection {
  from: string;    // Activity ID, or '__trigger__'
  to: string;      // Activity ID, or '__emit__'
  label?: string;  // Branch label for branching nodes: "True","False","Each item","Done"
}
```

**Special node IDs:**
- `__trigger__` — the workflow's trigger node (always exists)
- `__emit__` — the workflow's emit node (exists only when `workflow.emit` is set)

### 1.5 WorkflowRun

Execution history for a workflow.

```typescript
interface WorkflowRun {
  id: string;                          // "wr1", "wr2", etc.
  workflowId: string;                  // "morning-brief", "pr-review"
  status: 'success' | 'failed' | 'skipped';
  startedAt: string;                   // "Today, 8:00 AM"
  completedAt: string;                 // "Today, 8:02 AM"
  duration: string;                    // "2m 14s", "0m 45s"
  steps: number;                       // Total steps executed
  triggerType: string;                 // "schedule", "event", "manual"
  error?: string;                      // Error message if failed/skipped
  tokens: {
    input: number;                     // Token count
    output: number;
  };
  activities: ActivityRun[];           // Per-activity execution details
}

interface ActivityRun {
  id: string;                          // Activity ID: "scan-sources"
  status: 'success' | 'failed' | 'skipped';
  duration: string;                    // "1m 32s", "—" (if skipped)
  output?: string;                     // Result summary: "Found 14 articles across 6 sources"
  error?: string;                      // Error message if failed
}
```

**Storage key:** `"agentId:workflowId"` → e.g., `"researcher:morning-scan"`, `"coder:pr-review"`

### 1.6 WorkflowStats

Aggregate stats per agent.

```typescript
interface WorkflowStats {
  totalRuns: number;
  completed: number;
  failed: number;
  running: number;
  avgDuration: string;                 // "1m 48s" or "—"
  lastRunAt: string;                   // "Today, 8:02 AM" or "—"
}
```

---

## 2. Activity Type System

12 built-in types. Each type defines default skills, steps, parameters, and visual identity.

### 2.1 Type Enum

```typescript
type ActivityType =
  | 'custom'     | 'research'  | 'email'     | 'notify'
  | 'code'       | 'condition' | 'loop'      | 'wait'
  | 'agent'      | 'connector' | 'http'      | 'transform';
```

### 2.2 Type Definitions

Each type has a `ActivityTypeDefinition`:

```typescript
interface ActivityTypeDefinition {
  type: ActivityType;
  label: string;
  description: string;
  icon: string;                        // Single character: ◆ ⊕ ✉ ⊘ ⌘ ⑂ ↻ ⏸ ◉ ⊞ ⇄ ⊿
  accentClass: string;                 // DaisyUI border color class
  defaultSkills: string[];             // Pre-populated when creating
  defaultSteps: string[];              // Pre-populated when creating
  parameters: ActivityParameter[];     // Type-specific config fields
  branches?: boolean;                  // If true, creates branching outputs
  branchLabels?: string[];             // Branch output labels
}

interface ActivityParameter {
  key: string;
  label: string;
  type: 'text' | 'textarea' | 'select' | 'number' | 'toggle';
  placeholder?: string;
  description?: string;
  options?: Array<{ value: string; label: string }>;
  default?: string | number | boolean;
}
```

### 2.3 Complete Type Reference

| Type | Icon | Accent | Branching | Default Skills | Parameters |
|------|------|--------|-----------|----------------|------------|
| `custom` | ◆ | `border-base-300` | No | — | — |
| `research` | ⊕ | `border-success` | No | `@nebo/skills/web-scraper@^1.0.0` | `depth` (select: quick/standard/deep), `sources` (text) |
| `email` | ✉ | `border-info` | No | `@nebo/skills/gws-gmail@^1.0.0` | `to` (text), `subject` (text) |
| `notify` | ⊘ | `border-warning` | No | `@nebo/skills/slack@^1.0.0` | `channel` (select: slack/email/webhook), `target` (text) |
| `code` | ⌘ | `border-secondary` | No | `@nebo/skills/sandbox@^1.0.0` | `language` (select: javascript/python/typescript/shell), `code` (textarea) |
| `condition` | ⑂ | `border-accent` | **Yes**: `[True, False]` | — | `expression` (text), `mode` (select: expression/contains/exists/regex) |
| `loop` | ↻ | `border-accent` | **Yes**: `[Each item, Done]` | — | `source` (text), `maxIterations` (number, default: 100) |
| `wait` | ⏸ | `border-base-content/30` | No | — | `duration` (select: 5s/30s/1m/5m/15m/1h/custom), `waitUntil` (text) |
| `agent` | ◉ | `border-primary` | No | — | `agentId` (select), `instructions` (textarea) |
| `connector` | ⊞ | `border-primary` | No | — | `serverId` (select), `tool` (text), `input` (textarea) |
| `http` | ⇄ | `border-info` | No | — | `method` (select: GET/POST/PUT/PATCH/DELETE), `url` (text), `body` (textarea), `headers` (textarea) |
| `transform` | ⊿ | `border-secondary` | No | — | `operation` (select: map/filter/reduce/pick/template), `expression` (textarea) |

### 2.4 Branching Semantics

Only `condition` and `loop` create branching outputs:

**Condition** — two output edges, one labeled `"True"`, one `"False"`:
```
             ┌─ True ──→ [urgent-alert]
[check-urgency]
             └─ False ─→ [digest]
```

**Loop** — two output edges, one labeled `"Each item"`, one `"Done"`:
```
               ┌─ Each item ─→ [process-item]
[iterate-list]
               └─ Done ───────→ [summarize]
```

Both branches can merge back into a single downstream node. The layout engine handles Y-axis spreading of branch subtrees.

---

## 3. Graph Topology

### 3.1 Connection Rules

- Every workflow starts with `__trigger__`
- `__trigger__` connects to the first activity
- Activities connect to each other via `connections[]`
- Terminal activities can connect to `__emit__` (if emit is enabled)
- A node can have multiple outgoing edges (branching/parallel)
- A node can have multiple incoming edges (merge point)
- Self-loops are prevented by the UI
- Duplicate edges (same from+to) are prevented

### 3.2 Linear Fallback

When `connections` is `undefined`/absent, the frontend generates a linear chain:
```
__trigger__ → activity[0] → activity[1] → ... → activity[n] → __emit__
```

The function `generateLinearConnections(activities, emit)` produces this. Once any non-linear operation happens (branching, parallel path, manual connection), explicit `connections[]` is stored.

### 3.3 Parallel Paths

Created when a node has 2+ outgoing edges without branch labels:
```
             ┌──→ [review]
[read-diff] ─┤
             └──→ [security-scan]
```

Both paths can merge into a single downstream node:
```
[review] ──────────┐
                   ├──→ [summarize]
[security-scan] ───┘
```

### 3.4 Node Deletion Semantics

When deleting a node:
1. Find all parent connections (`c.to === nodeId`)
2. Find all child connections (`c.from === nodeId`)
3. Remove all connections involving the node
4. Re-connect each parent to each child (auto-heal the graph)

---

## 4. API Surface — Required Endpoints

Based on all frontend operations, the backend needs these endpoints:

### 4.1 Workflow CRUD

| Operation | Frontend Action | Data |
|-----------|----------------|------|
| **List agent workflows** | Page load at `/{agentId}/settings/workflows` | Returns `Record<string, Workflow>` |
| **Get single workflow** | Click workflow card | Returns `Workflow` by name |
| **Create workflow** | "+ New workflow" button | `{ name: string, trigger: { type: 'manual' }, description: '', isActive: true, activities: [] }` |
| **Update workflow** | Save in editor modal or canvas | Full `Workflow` object |
| **Delete workflow** | Delete button in canvas/modal | By name |
| **Toggle active** | Toggle switch on card/modal | `{ isActive: boolean }` |

### 4.2 Activity CRUD

All operations are within a workflow context:

| Operation | Frontend Action | Data |
|-----------|----------------|------|
| **Add activity** | Type picker or canvas catalog | `Activity` object inserted at position |
| **Update activity** | Edit in config panel or modal | Partial `Activity` patch (field + value) |
| **Remove activity** | Delete button, context menu, Delete key | Activity ID (connections auto-healed) |
| **Duplicate activity** | Duplicate button, context menu | Source activity ID (new ID generated) |
| **Reorder activity** | Move up/down buttons (text editor) | Activity index + direction (-1/+1) |
| **Change type** | Type dropdown | Activity ID + new `ActivityType` |

### 4.3 Connection CRUD

| Operation | Frontend Action | Data |
|-----------|----------------|------|
| **Add connection** | Wire drag, "+ Add connection" form | `{ from: string, to: string, label?: string }` |
| **Remove connection** | Click edge → Delete, context menu, × button | `{ from: string, to: string }` |
| **Generate linear** | Implicit when no connections exist | Called automatically from activity array order |

### 4.4 Trigger Updates

| Operation | Frontend Action | Data |
|-----------|----------------|------|
| **Set trigger type** | Click trigger type button | `{ type: 'schedule' \| 'heartbeat' \| 'event' \| 'manual' }` |
| **Set schedule** | Time picker + day presets | Full trigger object with `schedule` string |
| **Set heartbeat** | Interval dropdown + window toggle | Full trigger object with `interval` + optional `window` |
| **Set event** | Event source text input | Full trigger object with `event` string |

### 4.5 Workflow Runs (Read-Only)

| Operation | Frontend Action | Data |
|-----------|----------------|------|
| **List runs for agent** | Settings page stats cards | `WorkflowStats` aggregate |
| **List runs for workflow** | Run history in detail pane | `WorkflowRun[]` keyed by `"agentId:workflowId"` |
| **Get run detail** | Expand run in timeline | Single `WorkflowRun` with `activities[]` |

### 4.6 Emit / Event System

| Operation | Frontend Action | Data |
|-----------|----------------|------|
| **Set emit** | Toggle emit switch, edit emit input | `{ emit: string \| undefined }` |
| **Listen for event** | Set trigger type to `event` + event name | Trigger's `event` field |

Events create cross-workflow chains. The UI auto-generates emit names: workflow name → kebab → dots + `.completed`:
```
"Morning Brief" → "morning.brief.completed"
```

---

## 5. Node Catalog

The frontend provides a catalog of node types for the canvas drag-and-drop:

### 5.1 Catalog Item Shape

```typescript
interface CatalogItem {
  type: string;          // e.g. "activity-research", "flow-condition", "trigger-schedule", "emit"
  label: string;
  desc: string;
  icon: string;
  serverId?: string;     // For connector type
  serverName?: string;   // For connector type
  agentId?: string;      // For agent delegation type
  agentColor?: string;   // For agent delegation type
}
```

### 5.2 Type Prefix Mapping

The `type` field on catalog items maps to `ActivityType` via these prefixes:

| Catalog `type` prefix | Maps to `ActivityType` | Example |
|----------------------|----------------------|---------|
| `activity-*` | Strip prefix | `activity-research` → `research` |
| `flow-*` | Strip prefix | `flow-condition` → `condition` |
| `agent-*` | Always `agent` | `agent-coder` → `agent` |
| `connector-*` | Always `connector` | `connector-slack` → `connector` |
| `trigger-*` | Updates trigger type | `trigger-schedule` → updates `workflow.trigger` |
| `emit` | Enables emit | Sets `workflow.emit` |

### 5.3 Categories

```
Triggers:     schedule, event, heartbeat, manual
Activities:   custom, research, email, notify, code, http, transform
Flow Control: condition, loop, wait
Connectors:   Dynamic from enabled MCP servers
Agents:       Dynamic from agent roster (excluding assistant)
Output:       emit
```

---

## 6. Layout Engine

Used by the canvas to position nodes. **This runs entirely in the frontend** — the backend does not need to store positions.

### 6.1 Algorithm

1. Build adjacency list from `connections[]` (or linear fallback)
2. Create node objects: trigger (140×56), activities (240×88), emit (140×56)
3. Run dagre (Sugiyama algorithm) with `rankdir: 'LR'`, `nodesep: 80`, `ranksep: 128`
4. Read X positions from dagre
5. Post-process Y: spread branching children vertically around parent center
6. Build edges between nodes

### 6.2 Position Overrides

Users can drag nodes to custom positions. These are stored as `posOverrides: Record<nodeId, {x, y}>` in client state only. Overrides clear when the graph structure changes (nodes added/removed).

---

## 7. Component Architecture

### 7.1 File Map

```
src/lib/
├── utils/
│   ├── workflowTypes.ts          # Type system: 12 types, parameters, helpers
│   └── workflowLayout.ts         # DAG layout engine, mutation helpers, math
├── components/workflow/
│   ├── WorkflowBuilder.svelte    # Top-level orchestrator (canvas mode)
│   ├── BuilderCanvas.svelte      # SVG/HTML canvas with drag, zoom, pan
│   ├── NodeConfigPanel.svelte    # Right sidebar for editing selected node
│   ├── NodeCatalog.svelte        # Right panel for adding nodes
│   └── BuilderChat.svelte        # Left panel AI architect chat
└── mockData.ts                   # Static mock data (to be replaced by API)

src/routes/[agentId]/
├── +layout.svelte                # Agent context, workflow modal, canvas overlay
└── settings/[section]/
    └── +page.svelte              # Settings page (workflows section = text editor)
```

### 7.2 Data Flow

```
AGENT_CONFIGS (mockData)
    │
    ▼
[agentId]/+layout.svelte          ─── provides context via setContext('agentPage')
    │                                    │
    ├── editingWorkflow state ───────→ Workflow Editor Modal (text editor)
    │                                    ├── Type selector (12 types)
    │                                    ├── Activity CRUD
    │                                    ├── Connection CRUD
    │                                    └── Trigger/emit config
    │
    └── showCanvasModal state ───────→ WorkflowBuilder.svelte
                                         ├── builderWorkflows (deep clone, mutable)
                                         ├── undo/redo stack (JSON snapshots)
                                         ├── validation errors
                                         │
                                         ├── BuilderChat ──→ onaction callback
                                         ├── BuilderCanvas ──→ node/edge callbacks
                                         ├── NodeConfigPanel ──→ update callbacks
                                         └── NodeCatalog ──→ onselect callback
```

### 7.3 WorkflowBuilder Props

```typescript
{
  workflows: Record<string, Workflow>   // All workflows for the agent
  agentId: string
  agentName: string
  onclose?: () => void
  onsave?: (workflows: Record<string, Workflow>) => void
}
```

### 7.4 BuilderCanvas Props (Callbacks)

```typescript
{
  workflow: Workflow                           // Single workflow object
  workflowName: string
  agentId: string
  mode: 'view' | 'edit'
  selectedNodeId: string | null
  onselect?: (nodeId: string | null) => void
  onopenCatalog?: (afterNodeId: string | null, branchLabel?: string) => void
  onremove?: (nodeId: string) => void
  onduplicate?: (nodeId: string) => void
  oncreateConnection?: (fromId: string, toId: string) => void
  onremoveConnection?: (fromId: string, toId: string) => void
  ondropNode?: (item: CatalogItem, afterNodeId: string | null) => void
}
```

### 7.5 NodeConfigPanel Props (Callbacks)

```typescript
{
  workflowName: string
  workflow: Workflow
  selectedNodeId: string | null
  activity: Activity | null
  mode: 'view' | 'edit'
  onupdateActivity?: (field: string, value: any) => void
  onupdateTrigger?: (trigger: WorkflowTrigger) => void
  onupdateEmit?: (emit: string) => void
  onupdateDescription?: (desc: string) => void
  onremove?: (nodeId: string) => void
  onremoveWorkflow?: () => void
  onclose?: () => void
  onselectActivity?: (id: string) => void
}
```

---

## 8. Validation Rules

Applied in `WorkflowBuilder` before save:

1. **No empty activity IDs** — every activity must have a non-empty `id`
2. **No duplicate activity IDs** — within the same workflow
3. **No empty intents** — every activity must have a non-empty `intent`

Validation errors are displayed as a warning count in the toolbar and block the Save button.

---

## 9. Undo/Redo

- Implemented as JSON snapshot stack in `WorkflowBuilder`
- Every mutation calls `pushUndoSnapshot()` which serializes `builderWorkflows` to JSON
- `undo()` decrements pointer, `redo()` increments
- `Cmd+Z` / `Cmd+Shift+Z` keyboard shortcuts
- Discard resets to original snapshot

---

## 10. Existing Mock Data — Exact Shapes

### 10.1 Example Workflow with Branching (researcher:morning-scan)

```json
{
  "trigger": { "type": "schedule", "schedule": "8:00 AM daily" },
  "description": "Scan configured topics for overnight developments",
  "isActive": true,
  "lastFired": "Today, 8:00 AM",
  "emit": "research.digest.ready",
  "activities": [
    {
      "id": "scan-sources",
      "type": "research",
      "intent": "Check all configured topic sources",
      "skills": ["@nebo/skills/web-scraper@^1.0.0"],
      "steps": ["Search news for each configured topic", "Check competitor websites", "Scan industry publications"]
    },
    {
      "id": "check-urgency",
      "type": "condition",
      "intent": "Check if urgent findings exist",
      "skills": [],
      "steps": [],
      "params": { "expression": "data.findings.some(f => f.urgency === \"high\")", "mode": "expression" }
    },
    {
      "id": "urgent-alert",
      "type": "notify",
      "intent": "Send urgent Slack alert",
      "skills": ["@nebo/skills/slack@^1.0.0"],
      "steps": ["Format urgent findings for Slack", "Send to #alerts channel"],
      "params": { "channel": "slack", "target": "#alerts" }
    },
    {
      "id": "digest",
      "intent": "Compile findings into a digest",
      "skills": [],
      "steps": ["Rank findings by relevance", "Write summary per item", "Deliver to owner"]
    }
  ],
  "connections": [
    { "from": "__trigger__", "to": "scan-sources" },
    { "from": "scan-sources", "to": "check-urgency" },
    { "from": "check-urgency", "to": "urgent-alert", "label": "True" },
    { "from": "check-urgency", "to": "digest", "label": "False" },
    { "from": "urgent-alert", "to": "digest" },
    { "from": "digest", "to": "__emit__" }
  ]
}
```

### 10.2 Example Workflow with Parallel Paths (coder:pr-review)

```json
{
  "trigger": { "type": "event", "event": "GitHub PR opened" },
  "description": "Automated code review on new pull requests",
  "isActive": true,
  "lastFired": "Today, 10:15 AM",
  "emit": "code.review.complete",
  "activities": [
    { "id": "read-diff", "type": "code", "intent": "Read and understand the PR diff", "skills": ["@nebo/skills/github@^1.0.0"], "steps": ["Fetch the PR diff", "Identify changed modules"] },
    { "id": "review", "type": "code", "intent": "Perform code review", "skills": ["@nebo/skills/code-review@^1.0.0"], "steps": ["Check for correctness", "Verify style guide", "Scan for security issues", "Post inline comments"] },
    { "id": "security-scan", "type": "code", "intent": "Run security analysis", "skills": ["@nebo/skills/code-review@^1.0.0"], "steps": ["Scan for dependency vulnerabilities", "Check for secret exposure", "Verify input validation"] },
    { "id": "summarize", "type": "notify", "intent": "Post review summary", "skills": ["@nebo/skills/github@^1.0.0"], "steps": ["Write summary comment", "Tag owner if blocking"] }
  ],
  "connections": [
    { "from": "__trigger__", "to": "read-diff" },
    { "from": "read-diff", "to": "review" },
    { "from": "read-diff", "to": "security-scan" },
    { "from": "review", "to": "summarize" },
    { "from": "security-scan", "to": "summarize" },
    { "from": "summarize", "to": "__emit__" }
  ]
}
```

### 10.3 Example Workflow — Linear (assistant:morning-brief)

```json
{
  "trigger": { "type": "schedule", "schedule": "8:00 AM daily" },
  "description": "Morning briefing: overnight updates, today's agenda, pending items",
  "isActive": true,
  "lastFired": "Today, 8:00 AM",
  "emit": "brief.morning.delivered",
  "activities": [
    { "id": "gather-overnight", "intent": "Collect overnight updates from all agents", "skills": ["@nebo/skills/inbox@^1.0.0"], "steps": ["Check each agent's run history", "Collect flagged items", "Pull calendar for today"] },
    { "id": "compose-brief", "intent": "Write the morning briefing", "skills": [], "steps": ["Summarize overnight activity", "List meetings and deadlines", "Highlight items needing attention", "Deliver to owner"] }
  ]
}
```

No `connections` field → frontend infers: `__trigger__ → gather-overnight → compose-brief → __emit__`

### 10.4 Example Workflow — Heartbeat (coder:ci-monitor)

```json
{
  "trigger": { "type": "heartbeat", "interval": "30m", "window": { "start": "8:00 AM", "end": "6:00 PM" } },
  "description": "Monitor CI pipeline status",
  "isActive": true,
  "lastFired": "Today, 12:30 PM",
  "activities": [
    { "id": "check-pipelines", "type": "code", "intent": "Check all active CI pipelines", "skills": ["@nebo/skills/github@^1.0.0"], "steps": ["List running and recent pipelines", "Flag failures or long-running jobs"] }
  ]
}
```

### 10.5 Example Run

```json
{
  "id": "wr6",
  "workflowId": "morning-scan",
  "status": "success",
  "startedAt": "Today, 8:00 AM",
  "completedAt": "Today, 8:02 AM",
  "duration": "2m 14s",
  "steps": 4,
  "triggerType": "schedule",
  "tokens": { "input": 3800, "output": 1650 },
  "activities": [
    { "id": "scan-sources", "status": "success", "duration": "1m 32s", "output": "Found 14 articles across 6 sources" },
    { "id": "digest", "status": "success", "duration": "0m 42s", "output": "Digest compiled — 8 items, 2 flagged urgent" }
  ]
}
```

### 10.6 Example Failed Run

```json
{
  "id": "wr8",
  "workflowId": "morning-scan",
  "status": "failed",
  "startedAt": "Apr 26, 8:00 AM",
  "completedAt": "Apr 26, 8:00 AM",
  "duration": "0m 34s",
  "steps": 2,
  "triggerType": "schedule",
  "error": "Rate limit exceeded on news API — retried 3 times",
  "tokens": { "input": 820, "output": 0 },
  "activities": [
    { "id": "scan-sources", "status": "failed", "duration": "0m 34s", "error": "Rate limit exceeded on news API" },
    { "id": "digest", "status": "skipped", "duration": "—", "output": "Skipped — upstream dependency failed" }
  ]
}
```

### 10.7 Stats per Agent

```json
{
  "assistant":  { "totalRuns": 58,  "completed": 57,  "failed": 0, "running": 0, "avgDuration": "1m 42s",  "lastRunAt": "Today, 8:02 AM" },
  "researcher": { "totalRuns": 124, "completed": 119, "failed": 3, "running": 0, "avgDuration": "1m 48s",  "lastRunAt": "Today, 8:02 AM" },
  "coder":      { "totalRuns": 47,  "completed": 45,  "failed": 2, "running": 0, "avgDuration": "2m 56s",  "lastRunAt": "Today, 10:18 AM" },
  "marketer":   { "totalRuns": 68,  "completed": 67,  "failed": 1, "running": 0, "avgDuration": "2m 58s",  "lastRunAt": "Today, 9:02 AM" },
  "social":     { "totalRuns": 89,  "completed": 82,  "failed": 2, "running": 0, "avgDuration": "0m 38s",  "lastRunAt": "Today, 9:00 AM" },
  "ops":        { "totalRuns": 312, "completed": 308, "failed": 3, "running": 1, "avgDuration": "1m 12s",  "lastRunAt": "Today, 7:32 AM" },
  "tester":     { "totalRuns": 0,   "completed": 0,   "failed": 0, "running": 0, "avgDuration": "—",       "lastRunAt": "—" },
  "writer":     { "totalRuns": 0,   "completed": 0,   "failed": 0, "running": 0, "avgDuration": "—",       "lastRunAt": "—" }
}
```

---

## 11. All Agents with Workflows

| Agent ID | Workflows | Trigger Types Used |
|----------|-----------|-------------------|
| `assistant` | `morning-brief`, `evening-wrap` | schedule |
| `researcher` | `morning-scan`, `afternoon-trends`, `deep-dive` | schedule, manual |
| `coder` | `pr-review`, `deploy-notify`, `ci-monitor` | event, heartbeat |
| `marketer` | `weekly-report`, `content-calendar` | schedule |
| `social` | `morning-post`, `afternoon-post` | schedule |
| `ops` | `email-triage`, `calendar-sync` | event, schedule |
| `tester` | — | — |
| `writer` | — | — |

---

## 12. Cross-Workflow Event Chains

Workflows can be chained via emit/event triggers:

```
assistant:morning-brief  ──emit: "brief.morning.delivered"──→  (any workflow listening for this event)
researcher:morning-scan  ──emit: "research.digest.ready"──→    (any workflow listening)
coder:pr-review          ──emit: "code.review.complete"──→     (any workflow listening)
marketer:weekly-report   ──emit: "marketing.report.ready"──→   (any workflow listening)
social:morning-post      ──emit: "social.post.published"──→    (any workflow listening)
ops:email-triage         ──emit: "email.triaged"──→            (any workflow listening)
```

---

## 13. Skill URI Format

Skills are referenced by URI with semver range:

```
@nebo/skills/web-scraper@^1.0.0
@nebo/skills/slack@^1.0.0
@nebo/skills/gws-gmail@^1.0.0
@nebo/skills/gws-calendar@^1.0.0
@nebo/skills/github@^1.0.0
@nebo/skills/code-review@^1.0.0
@nebo/skills/deep-research@^1.0.0
@nebo/skills/content@^1.0.0
@nebo/skills/social@^1.0.0
@nebo/skills/analytics@^1.0.0
@nebo/skills/inbox@^1.0.0
@nebo/skills/sandbox@^1.0.0
```

---

## 14. Marketplace Integration

Workflows with `source: 'marketplace'` are read-only:
- Cannot edit trigger, activities, description, connections
- Cannot add/remove/reorder activities
- Can toggle `isActive` on/off
- Can duplicate as editable copy
- Shows "Marketplace" badge and lock icon

---

## 15. Mutation Helper Functions

These are the pure functions in `workflowLayout.ts` that the backend should replicate for server-side graph operations:

```typescript
// Add activity at position. afterId=null appends. afterId='__trigger__' prepends.
addActivityToWorkflow(activities: Activity[], afterId: string | null, newActivity?: Partial<Activity>): Activity[]

// Remove by ID
removeActivityFromWorkflow(activities: Activity[], activityId: string): Activity[]

// Deep clone + new ID
duplicateActivityInWorkflow(activities: Activity[], activityId: string): Activity[]

// Add edge (generates linear connections first if none exist)
addConnection(connections: WorkflowConnection[] | undefined, activities: Activity[], emit: string | undefined, from: string, to: string): WorkflowConnection[]

// Remove edge
removeConnection(connections: WorkflowConnection[], from: string, to: string): WorkflowConnection[]

// Generate linear chain from array order
generateLinearConnections(activities: Activity[], emit?: string): WorkflowConnection[]
```

---

## 16. Schedule Store Integration

The schedule store (`src/lib/stores/schedule.ts`) consumes workflow data to show calendar events:

- Reads `AGENT_CONFIGS` to extract scheduled workflows
- Parses trigger schedule strings into `{ hour, days }` for calendar placement
- Estimates duration from `MOCK_WORKFLOW_RUNS` averages
- Shows heartbeat workflows as consolidated daily entries
- Attaches run status (success/failed/skipped) per calendar date

The backend should expose an endpoint that returns scheduled workflows with their next/recent run times for calendar integration.

---

## 17. Summary — What the Backend Must Serve

| Endpoint | Returns | Used By |
|----------|---------|---------|
| `GET /agents/{id}/workflows` | `Record<string, Workflow>` | Settings page, canvas |
| `GET /agents/{id}/workflows/{name}` | `Workflow` | Editor modal |
| `PUT /agents/{id}/workflows/{name}` | Updated `Workflow` | Save from modal/canvas |
| `POST /agents/{id}/workflows` | New `Workflow` | Create workflow |
| `DELETE /agents/{id}/workflows/{name}` | — | Delete workflow |
| `PATCH /agents/{id}/workflows/{name}` | — | Toggle `isActive` |
| `GET /agents/{id}/workflow-stats` | `WorkflowStats` | Settings stats cards |
| `GET /agents/{id}/workflow-runs` | `WorkflowRun[]` | Run history |
| `GET /agents/{id}/workflow-runs/{runId}` | `WorkflowRun` | Run detail |
| `GET /catalog/node-types` | `ActivityTypeDefinition[]` | Node catalog |
| `GET /agents` | Agent list | Agent delegation catalog |
| `GET /mcp/servers` | MCP server list | Connector catalog |
