# Workflow Builder UI — SME Reference

Deep-dive reference for the Nebo Workflow Builder visual editor. Covers the frontend
component architecture, canvas rendering, node type system, data flow, state management,
API integration, and cross-system interactions with the Rust workflow engine.

---

## Architecture Overview

The Workflow Builder is a full-screen modal that opens over the agent layout. It provides
a visual DAG (directed acyclic graph) editor for composing workflow activities, a
configuration panel for each node, and an AI "Architect" chat for natural-language
workflow modification.

```
+----------------------------------------------------------------------+
|  Agent Layout ([agentId]/+layout.svelte)                             |
|                                                                      |
|  showCanvasModal = true                                              |
|  +------------------------------------------------------------------+
|  |  WorkflowBuilder.svelte (full-screen modal)                      |
|  |                                                                  |
|  |  +----------+  +-----------------------------+  +-------------+  |
|  |  | Builder  |  |     BuilderCanvas.svelte     |  |  NodeConfig |  |
|  |  | Chat     |  |                             |  |  Panel      |  |
|  |  | (320px)  |  |  +---------+  +---------+   |  |  (340px)    |  |
|  |  |          |  |  | Trigger |->| Act #1  |   |  |             |  |
|  |  | AI       |  |  +---------+  +---------+   |  | Trigger     |  |
|  |  | Architect|  |               |              |  | config      |  |
|  |  |          |  |          +---------+         |  | Activity    |  |
|  |  | "Add a   |  |          | Act #2  |->Emit   |  | detail      |  |
|  |  |  step.."|  |          +---------+         |  | Steps       |  |
|  |  |          |  |                             |  | Skills      |  |
|  |  +----------+  +-----------------------------+  +-------------+  |
|  |                                                                  |
|  |  [Toolbar: Undo | Redo | Add Node | Tidy Up | Errors | Save]     |
|  |  [Workflow tabs: wf-1 | wf-2 | +]                               |
|  +------------------------------------------------------------------+
+----------------------------------------------------------------------+
```

### Component Tree

```
[agentId]/+layout.svelte
  |
  +-- WorkflowBuilder.svelte          (state owner, mutation handlers)
       |
       +-- BuilderChat.svelte          (left panel — AI architect)
       |    |
       |    +-- ChatPane.svelte        (reuses shared chat component)
       |
       +-- BuilderCanvas.svelte        (center — SVG + HTML canvas)
       |
       +-- NodeConfigPanel.svelte      (right panel — config form)
       |
       +-- NodeCatalog.svelte          (slide-in catalog overlay)
```

### Supporting Modules

```
app/src/lib/utils/
  +-- workflowLayout.ts    Layout engine (dagre), edge paths, mutation helpers
  +-- workflowTypes.ts     Activity type system, catalog mapping, defaults

app/src/lib/types/
  +-- agentPage.ts         WorkflowConfig, WorkflowActivity, WorkflowTrigger

app/src/lib/tokens.ts      NODE_CATALOG_ITEMS, ARCHITECT_INTRO_MESSAGE

app/src/lib/components/
  +-- WorkflowCanvas.svelte   Read-only canvas (view mode, used in runs/schedule)
```

---

## Entry Points

The Workflow Builder is opened from two locations:

1. **Agent Settings > Workflows section** — clicking "Workflow Builder" or individual
   workflow entries calls `ctx.openCanvas()` or `ctx.openWorkflow(name, wf)`, which
   sets `showCanvasModal = true` in `[agentId]/+layout.svelte`.

2. **Schedule page** — the schedule view embeds `WorkflowBuilder` for workflow editing
   context alongside the calendar.

The builder receives these props from the parent layout:

```typescript
workflows: Record<string, WorkflowConfig>   // full workflow map from agent config
agentId: string                              // current agent ID
agentName: string                            // display name
onclose: () => void                          // close modal
onsave: (wfs: Record<string, WorkflowConfig>) => void  // persist changes
```

On save, the parent layout writes back to `config.workflows` which triggers the
agent update API call upstream.

---

## Data Model

### Frontend Types (agentPage.ts)

```typescript
interface WorkflowTrigger {
    type: string              // 'schedule' | 'heartbeat' | 'event' | 'manual'
    event?: string            // event source pattern (e.g. "email.urgent")
    schedule?: string         // human-readable: "8:00 AM weekdays"
    interval?: string         // heartbeat interval: "5m", "1h", etc.
    window?: { start?: string; end?: string }  // active hours for heartbeat
}

interface WorkflowConfig {
    trigger?: WorkflowTrigger
    schedule?: string         // legacy: top-level schedule string
    activities?: WorkflowActivity[]
    connections?: { from: string; to: string; label?: string }[]
    source?: string
    isActive?: boolean
    description?: string
    lastFired?: string
    emit?: string             // event emitted on completion
}

interface WorkflowActivity {
    id: string                // unique within workflow
    type: string              // ActivityType enum value
    label?: string
    description?: string
    tool?: string
    resource?: string
    action?: string
    params?: Record<string, unknown>  // type-specific parameters
    branches?: { label: string; nextId?: string }[]
    intent?: string           // natural-language task description
    skills?: string[]         // qualified skill names
    steps?: string[]          // ordered execution steps
}
```

### Connection Model

Connections define the DAG edges explicitly. Each connection has:
- `from`: source node ID (or `__trigger__` for the trigger pseudo-node)
- `to`: target node ID (or `__emit__` for the emit pseudo-node)
- `label`: optional branch label (e.g. "True", "False", "Each item", "Done")

When `connections` is absent or empty, the layout engine falls back to **linear
chain order** derived from the `activities` array index.

### Backend Data Model (parser.rs)

The Rust `WorkflowDef` mirrors the frontend but with additional execution metadata:

```rust
struct WorkflowDef {
    version: String,
    id: String,
    name: String,
    inputs: HashMap<String, InputParam>,
    activities: Vec<Activity>,
    dependencies: Dependencies,
    budget: Budget,
}

struct Activity {
    id: String,
    intent: String,
    skills: Vec<String>,
    mcps: Vec<String>,          // MCP server dependencies
    cmds: Vec<String>,          // workflow commands (e.g. "emit")
    model: String,              // LLM model override
    steps: Vec<String>,
    token_budget: TokenBudget,
    on_error: OnError,          // retry count + fallback policy
    min_iterations: u32,        // budget continuation threshold
}
```

Key difference: triggers are NOT stored in the workflow definition. They are owned
by agents via `agent_workflows` bindings. The frontend `WorkflowConfig.trigger` is an
agent-level concept that the builder manages.

---

## Canvas Rendering

### Layout Engine (workflowLayout.ts)

The layout engine uses **dagre** (`@dagrejs/dagre`) to compute node positions via
the Sugiyama algorithm (layered graph drawing). This runs entirely client-side as a
derived computation — no server round-trips for layout.

```
Layout Pipeline:
  WorkflowConfig
    |
    v
  buildAdjacency()       -- connections[] -> adjacency map, or linear fallback
    |
    v
  dagre.layout(g)        -- rank assignment + horizontal/vertical spacing
    |                        rankdir: 'LR' (left-to-right)
    |                        nodesep: 80, ranksep: 128, edgesep: 48
    v
  Y-override pass        -- spread branching children vertically around parent
    |
    v
  { nodes[], edges[] }   -- LayoutWorkflowNode[] + WorkflowEdge[]
```

**Node dimensions:**
- Activity nodes: 240px wide x 88px tall (`NODE_W`, `NODE_H`)
- Trigger/Emit nodes: 140px wide x 56px tall (`TRIGGER_W`, `TRIGGER_H`)
- Gap between nodes: 60px horizontal, 24px vertical (`GAP_X`, `GAP_Y`)
- Canvas padding: 40px (`PADDING`)

**Branching layout:** After dagre computes initial positions, a second pass detects
fork nodes (nodes with 2+ outgoing edges). For each fork, child subtrees are spread
vertically around the parent's center with `NODE_H + GAP_Y` spacing. Subtree
collection uses DFS with merge-point detection — exclusive subtrees stop at nodes
that have parents outside the subtree.

### Rendering Layers

The canvas renders in three stacked layers:

```
Layer 0:  SVG dot-grid background pattern
          (20x20 grid, transforms with pan/zoom)

Layer 1:  SVG edge layer
          - Bezier curves (cubic) from node right-edge to next node left-edge
          - Arrow polygon at target
          - Edge labels (branch names)
          - Invisible 16px-wide hit areas for click detection
          - Wire drag preview (dashed line during connection creation)

Layer 2:  HTML node layer (div-based, CSS transforms)
          - Node cards with border colors per type/status
          - Output handles (drag to connect)
          - "+" connector buttons between edges
          - Context menus
```

### Edge Rendering (edgePath)

Edges are cubic Bezier curves connecting the right edge of the source node to the
left edge of the target node:

```
M {x1} {y1} C {x1+cp} {y1}, {x2-cp} {y2}, {x2} {y2}

where:
  x1 = from.x + from.w        (right edge of source)
  y1 = from.y + from.h / 2    (vertical center of source)
  x2 = to.x                   (left edge of target)
  y2 = to.y + to.h / 2        (vertical center of target)
  cp = min((x2 - x1) * 0.5, 40)  (control point offset)
```

---

## Node Type System (workflowTypes.ts)

### Activity Types

12 activity types organized into 4 categories:

| Category       | Type        | Icon | Accent         | Branches |
|----------------|-------------|------|----------------|----------|
| **Activities** | custom      | ◆    | border-base-300 | No      |
|                | research    | ⊕    | border-success  | No      |
|                | email       | ✉    | border-info     | No      |
|                | notify      | ⊘    | border-warning  | No      |
|                | code        | ⌘    | border-secondary| No      |
| **Flow**       | condition   | ⑂    | border-accent   | True/False |
|                | loop        | ↻    | border-accent   | Each item/Done |
|                | wait        | ⏸    | border-base-content/30 | No |
| **Integration**| agent       | ◉    | border-primary  | No      |
|                | connector   | ⊞    | border-primary  | No      |
|                | http        | ⇄    | border-info     | No      |
| **Data**       | transform   | ⊿    | border-secondary| No      |

### Type Definition Structure

Each `ActivityTypeDefinition` carries:

```typescript
{
    type: ActivityType,           // enum value
    label: string,                // display name
    description: string,          // tooltip/catalog description
    icon: string,                 // unicode icon
    accentClass: string,          // DaisyUI border color class
    defaultSkills: string[],      // pre-populated skills
    defaultSteps: string[],       // pre-populated steps template
    parameters: ActivityParameter[],  // type-specific config fields
    branches?: boolean,           // creates branching outputs
    branchLabels?: string[],      // output port labels
}
```

### Type-Specific Parameters

Each type defines its own parameter schema. Examples:

- **research**: `depth` (select: quick/standard/deep), `sources` (text)
- **email**: `to` (text), `subject` (text template with `{{topic}}`)
- **condition**: `expression` (text), `mode` (select: expression/contains/exists/regex)
- **loop**: `source` (text: data path), `maxIterations` (number, default 100)
- **connector**: `serverId` (select), `tool` (text), `input` (textarea, JSON)
- **http**: `method` (select: GET/POST/PUT/PATCH/DELETE), `url`, `body`, `headers`
- **code**: `language` (select: JS/Python/TS/Shell), `code` (textarea)
- **wait**: `duration` (select: 5s-1h), `waitUntil` (text: event name)

### Creating Typed Activities

`createTypedActivity(catalogType, catalogItem)` maps catalog selections to fully
initialized activity objects:

1. Maps catalog type string to `ActivityType` via prefix matching:
   - `activity-*` -> strip prefix, lookup in `ACTIVITY_TYPES`
   - `agent-*` -> `'agent'`
   - `connector-*` -> `'connector'`
   - `flow-*` -> strip prefix, lookup
   - fallback -> `'custom'`

2. Generates unique ID: `{label-slug}-{base36-timestamp}`

3. Pre-populates default params from type definition

4. Special handling for agent delegation (`agentId` param) and MCP connectors
   (`serverId` param)

---

## Node Catalog (NodeCatalog.svelte)

The catalog is a slide-in panel (300px wide, right side) that presents available
node types organized by category.

### Static Categories (from tokens.ts)

```
Triggers:           Schedule, Event, Heartbeat, Manual
Activities:         Custom, Research, Email, Notify, Code, HTTP, Transform
Flow Control:       Condition, Loop, Wait
Output:             Emit Event
```

### Dynamic Categories (populated at mount)

On mount, the catalog fetches:
1. `nebo.listIntegrations()` — connected MCP servers become **Connectors (MCP)** items
2. `nebo.listAgents()` — registered agents (excluding 'assistant') become **Agents** items

Each dynamic item carries metadata (`serverId`/`serverName` or `agentId`) that flows
into the created activity's params.

### Interaction Model

- **Click**: Selects the item and adds it to the workflow at the insertion point
- **Drag**: Items are draggable (`draggable="true"`) with MIME type
  `application/x-workflow-node` carrying the item JSON. Can be dropped onto the
  canvas (nearest node or end of chain).
- **Search**: Real-time filter across label and description fields

---

## Canvas Interactions (BuilderCanvas.svelte)

### Pan and Zoom

| Action         | Behavior                                  |
|----------------|-------------------------------------------|
| Scroll wheel   | Zoom toward cursor (factor 1.1/0.9)       |
| Click + drag   | Pan canvas (background only)              |
| Fit to screen  | Button in zoom controls (top-right corner) |
| Zoom in/out    | Buttons in zoom controls                  |
| Zoom range     | 0.3x to 2.0x (`MIN_ZOOM`, `MAX_ZOOM`)    |

Zoom applies via CSS `transform: translate(px, py) scale(z)` with `transform-origin: 0 0`.
The SVG edge layer uses an SVG `<g transform>` for the same values.

Auto-fit runs on first load and workflow tab switches via `fitToContainer()`, which
calculates the scale factor to fit the graph bounds within the container with 20px margin.

### Node Dragging

Nodes are draggable in edit mode:
1. `mousedown` on `[data-wf-node]` starts tracking
2. Position delta tracked in screen coords, divided by zoom for canvas coords
3. Drag threshold: 3px before committing (prevents accidental drags on click)
4. During drag: position override stored in `posOverrides` state
5. Edges recompute from overridden positions in real-time
6. If no drag occurred (< 3px), treated as click (select node)

### Wire Dragging (Connection Creation)

Each non-emit node has an output handle (`[data-wf-handle]`) on its right edge:
1. `mousedown` on handle starts wire drag
2. A dashed Bezier preview line follows the cursor
3. Hovering over a valid target node highlights it with a ring
4. `mouseup` on a target node: calls `oncreateConnection(sourceId, targetId)`
5. `mouseup` on empty canvas: opens catalog with `__parallel__` branch label
6. Pressing Escape cancels the wire drag

### Context Menus

Right-click on a node shows:
- **Add Parallel Path** — opens catalog with `__parallel__` branch label
- **Duplicate Node** — clones the node and inserts after
- **Delete Node** — removes with connection re-linking

Right-click on an edge shows:
- **Delete Connection** — removes the edge

### "+" Connector Buttons

Between every connected pair of nodes, a "+" button appears at the midpoint.
For terminal nodes (no outgoing edges), a "+" appears to the right.
For branching nodes, labeled "+" buttons appear for each unused branch label.

### Keyboard Shortcuts

| Key                | Action                    |
|--------------------|---------------------------|
| Delete / Backspace | Delete selected node/edge |
| Escape             | Deselect / cancel / close |
| Cmd+Z              | Undo                      |
| Cmd+Shift+Z        | Redo                      |
| Cmd+S              | Save                      |

### Catalog Drag-and-Drop

Canvas handles `dragover`/`drop` events for catalog items:
1. `dragover`: checks MIME type `application/x-workflow-node`, finds node under cursor
2. `drop`: parses JSON from dataTransfer, determines insertion point:
   - If dropped on a node: inserts after that node
   - If dropped on empty canvas: finds last terminal node, appends after it
3. Delegates to `ondropNode` callback

---

## Configuration Panel (NodeConfigPanel.svelte)

The right panel (340px) shows either:

### Workflow Overview (no node selected)

- **Trigger type** — 4-button grid (Schedule, Heartbeat, Event, Manual)
- **Trigger config** — type-specific:
  - Schedule: hour/minute/AM-PM pickers + day preset (Daily/Weekdays/Weekends/Custom)
  - Heartbeat: interval dropdown + optional time window (start/end)
  - Event: text input for comma-separated event source patterns
  - Manual: static text
- **Description** — textarea
- **Emit** — text input for event name emitted on completion
- **Last Fired** — timestamp (read-only)
- **Activity list** — clickable list of all activities (navigates to detail)
- **Delete Workflow** — button (edit mode only)

### Activity Detail (node selected)

- **Type badge** — icon + label + type selector dropdown
- **Activity ID** — editable text input
- **Intent** — textarea (natural-language task description)
- **Skills** — tag list with add/remove
- **Parameters** — type-specific fields rendered from `ActivityTypeDefinition.parameters`:
  - `text` -> input
  - `textarea` -> textarea
  - `select` -> dropdown
  - `number` -> number input
  - `toggle` -> checkbox
- **Steps** — ordered list with:
  - Inline editing (click to edit, Enter to save, Escape to cancel)
  - Add new step input at bottom
  - Delete button (appears on hover)
- **Delete Node** — button (edit mode only)
- **Back to workflow overview** — footer link

---

## State Management

### Mutable Builder State

The builder operates on a **deep clone** of the input workflows:

```typescript
const originalSnapshot = $derived(JSON.stringify(workflows));      // immutable reference
let builderWorkflows = $state(JSON.parse(JSON.stringify(workflows))); // mutable copy
```

All mutations go through `updateActiveWorkflow(updater)`:
1. Deep-clones the active workflow
2. Applies the updater function
3. Replaces in `builderWorkflows`
4. Pushes undo snapshot

### Dirty Tracking

```typescript
const isDirty = $derived(JSON.stringify(builderWorkflows) !== originalSnapshot);
```

If dirty when closing, a `confirm()` dialog warns about unsaved changes.

### Undo/Redo

JSON-snapshot-based undo with a pointer into a stack:

```typescript
let undoStack = $state<string[]>([originalSnapshot]);
let undoPointer = $state(0);

function pushUndoSnapshot() {
    const snap = JSON.stringify(builderWorkflows);
    undoStack = [...undoStack.slice(0, undoPointer + 1), snap];
    undoPointer = undoStack.length - 1;
}

function undo() {
    undoPointer--;
    builderWorkflows = JSON.parse(undoStack[undoPointer]);
}

function redo() {
    undoPointer++;
    builderWorkflows = JSON.parse(undoStack[undoPointer]);
}
```

Every mutation that calls `updateActiveWorkflow()` automatically pushes a snapshot.
Undo/redo truncates the stack at the current pointer (standard undo semantics).

### Selection State

```typescript
let selectedNodeId = $state<string | null>(null);     // activity ID or null
let selectedEdgeKey = $state<string | null>(null);     // "fromId->toId" or null
let mode = $state<'view' | 'edit'>('edit');            // view/edit toggle
let chatOpen = $state(true);                           // architect panel visibility
let catalogOpen = $state(false);                       // node catalog visibility
let catalogInsertAfter = $state<string | null>(null);  // insertion point
let catalogInsertBranchLabel = $state<string | null>(null);
```

### Canvas State (BuilderCanvas)

```typescript
let pan = $state({ x: 0, y: 0 });         // pixel offset
let zoom = $state(1);                       // scale factor
let panning = $state(false);                // background drag active
let posOverrides = $state<Record<string, {x, y}>>({});  // node drag overrides
let nodeDrag = $state(null);                // active node drag
let wireDrag = $state(null);                // active wire drag
let contextMenu = $state(null);             // right-click menu position + target
```

---

## Validation

Frontend validation runs as a derived computation on every state change:

```typescript
interface ValidationError {
    workflowName: string;
    nodeId?: string;
    message: string;
}
```

**Checks performed:**
1. **Empty activity ID** — every activity must have a non-empty `id`
2. **Duplicate activity ID** — IDs must be unique within a workflow
3. **Missing intent** — every activity must have a non-empty `intent`

Validation errors are displayed in the toolbar as a warning badge showing the count.
Hovering shows the full list of error messages. The **Save button is disabled** when
errors exist.

Backend validation (parser.rs) performs additional checks:
- Workflow `id` and `name` are required
- At least one activity is required
- Activity IDs are unique
- Token budgets sum does not exceed `total_per_run`

---

## AI Architect Chat (BuilderChat.svelte)

The left panel (320px) provides a simulated AI assistant for workflow modification.

### Current Implementation

The chat is **locally simulated** (no backend LLM call). It uses pattern matching
on user input to determine actions:

| User Intent         | Pattern Match                      | Action                    |
|---------------------|------------------------------------|---------------------------|
| Add/create activity | "add", "create", "new"             | Extracts label + intent, calls `onaction('add-activity', ...)` |
| Delete node         | "delete", "remove"                 | Instructions to right-click |
| Connect workflows   | "connect", "chain", "link"         | Explains emit/event chaining |
| Configure trigger   | "trigger", "schedule", "when"      | Explains trigger config panel |
| Other               | fallback                           | Lists capabilities |

### Message Flow

```
User types message
  |
  v
handleSend(text)
  |
  +-- Add user message to messages[]
  +-- Pattern match on lowercase text
  +-- setTimeout (simulated thinking)
  +-- Add thinking block message
  +-- setTimeout (simulated processing)
  +-- Add tool-group message (simulated tool call)
  +-- Add assistant response message
  +-- Call onaction() to mutate workflow (if applicable)
```

The chat reuses the shared `ChatPane` component with a custom agent name ("Architect")
and a static intro message from `ARCHITECT_INTRO_MESSAGE` in tokens.ts.

---

## API Endpoints

Workflows are persisted through the agent workflow binding API:

### Agent-Level Workflow APIs

```
GET    /api/v1/agents/{id}/workflows              List agent workflows
POST   /api/v1/agents/{id}/workflows              Create agent workflow
PUT    /api/v1/agents/{id}/workflows/{binding}     Update agent workflow
DELETE /api/v1/agents/{id}/workflows/{binding}     Delete agent workflow
POST   /api/v1/agents/{id}/workflows/{binding}/toggle   Toggle active state
POST   /api/v1/agents/{id}/workflows/{name}/run    Run workflow manually
```

### Standalone Workflow APIs (marketplace/shared)

```
GET    /api/v1/workflows                    List workflows
POST   /api/v1/workflows                    Create workflow
GET    /api/v1/workflows/{id}               Get workflow
PUT    /api/v1/workflows/{id}               Update workflow
DELETE /api/v1/workflows/{id}               Delete workflow
GET    /api/v1/workflows/{id}/bindings      List bindings
PUT    /api/v1/workflows/{id}/bindings      Update bindings
POST   /api/v1/workflows/{id}/run           Run workflow
GET    /api/v1/workflows/{id}/runs          List runs
GET    /api/v1/workflows/{id}/runs/{runId}  Get run
POST   /api/v1/workflows/{id}/runs/{runId}/cancel  Cancel run
POST   /api/v1/workflows/{id}/toggle        Toggle workflow
```

### Save Flow

```
WorkflowBuilder.onsave(wfs)
  |
  v
Parent layout: config.workflows = wfs
  |
  v
Agent config save (upstream) -> PUT /api/v1/agents/{id}
  |
  v
Server handler -> updates agent_workflows table
  |
  v
Trigger registration (triggers.rs) -> upserts cron_jobs for schedules
```

---

## Cross-System Interactions

### Workflow Builder -> Workflow Engine

The visual builder produces `WorkflowConfig` objects stored in the agent's config.
When a workflow is triggered (manually, by schedule, or by event), the server:

1. Reads the inline workflow definition from the `agent_workflows` table
2. Parses it through `parser::parse_workflow()` into a `WorkflowDef`
3. Validates via `parser::validate_workflow()`
4. Executes via `engine::execute_workflow()`

### Trigger System

```
Frontend (WorkflowBuilder)
  |  trigger type + config (schedule/heartbeat/event/manual)
  v
Agent API (save)
  |
  v
agent_workflows table
  |  trigger_type, trigger_config, is_active
  v
triggers.rs::register_agent_triggers()
  |
  +-- schedule: creates cron_job (name: "agent-{id}-{binding}")
  +-- event: stored in agent_workflows, consumed by EventDispatcher
  +-- heartbeat: server heartbeat loop checks interval + window
  +-- manual: no registration needed
```

### Event System

When a workflow has `emit` configured:

```
Workflow completes
  |
  v
Last activity calls emit tool
  |
  v
EventBus publishes Event { source, payload }
  |
  v
EventDispatcher.match_event()
  |  pattern matching (exact or wildcard "email.*")
  v
Matching subscriptions -> run_inline() for each
```

This enables workflow chaining: Workflow A emits `brief.delivered`, Workflow B
triggers on event `brief.delivered`.

### Read-Only Canvas (WorkflowCanvas.svelte)

A separate `WorkflowCanvas.svelte` component provides a **read-only** view of
workflows. Used in:
- Run detail pages (showing workflow graph with execution status)
- Schedule overview

This component:
- Accepts `workflows: Record<string, WorkflowConfig>` (multi-workflow support)
- Stacks multiple workflows vertically with 60px spacing
- Supports pan, zoom, fit-to-screen
- Click selects nodes and shows a detail panel (read-only)
- No editing capabilities (no drag, no catalog, no wire creation)
- Shows execution status via node border/background colors:
  - `success` -> border-success / bg-success/5
  - `failed` -> border-error / bg-error/5
  - `running` -> border-warning (with pulse animation)

---

## View vs Edit Mode

The builder has a View/Edit toggle in the toolbar:

| Feature                  | View Mode | Edit Mode |
|--------------------------|-----------|-----------|
| Node selection           | Yes       | Yes       |
| Pan/zoom                 | Yes       | Yes       |
| Node dragging            | No        | Yes       |
| Wire creation            | No        | Yes       |
| "+" connector buttons    | No        | Yes       |
| Context menus            | No        | Yes       |
| Output handles           | No        | Yes       |
| Config panel editing     | No        | Yes       |
| Architect chat           | No        | Yes       |
| Toolbar save/discard     | No        | Yes       |
| Keyboard delete          | No        | Yes       |

---

## Connection Management

### Adding Connections

Connections are created through:

1. **Wire drag** — dragging from an output handle to a target node
2. **Node insertion** — when adding a node via catalog, connections are automatically
   wired:
   - If inserting after a node with outgoing edges: splices into the chain
     (parent -> new -> old-target)
   - If inserting after a terminal node: appends (parent -> new)
   - If inserting at end (no afterId): connects from last activity or trigger
   - Branch label support: if `branchLabel` is provided, the connection carries
     that label

3. **Parallel paths** — adding with `__parallel__` branch label creates a fork
   (parent -> new, without removing existing edges)

### Removing Connections

- Right-click edge -> "Delete Connection"
- Select edge (click) -> Delete/Backspace key
- Selected edge shows a red X badge at midpoint for click-to-delete

### Automatic Reconnection on Node Delete

When a node is deleted, its connections are re-linked:

```
Before: A -> [deleted] -> B
After:  A -> B

Before: A -> [deleted] -> B, A -> [deleted] -> C
After:  nothing (all edges removed)

Before: X -> [deleted] -> B, Y -> [deleted] -> B
After:  X -> B, Y -> B
```

---

## Workflow Mutation Helpers (workflowLayout.ts)

### addActivityToWorkflow(activities, afterId, newActivity)

Inserts a new activity at the correct position:
- `afterId === null` -> append to end
- `afterId === '__trigger__'` -> insert at index 0
- otherwise -> insert after the activity with matching ID

### removeActivityFromWorkflow(activities, activityId)

Filters out the activity with the given ID.

### duplicateActivityInWorkflow(activities, activityId)

Clones an activity with a new ID (`{originalId}-copy-{counter}`) and inserts
immediately after the original. Connections are spliced to include the duplicate.

### generateLinearConnections(activities, emit)

Generates a simple chain: `__trigger__ -> act[0] -> act[1] -> ... -> __emit__`.
Used as fallback when no explicit connections exist.

### removeConnection(connections, from, to)

Filters out the matching connection.

---

## Error Handling

### Frontend Validation Feedback

- Validation errors shown as warning badge in toolbar with count
- Hover tooltip lists all error messages
- Save button disabled when errors present
- Individual errors reference workflow name and node ID

### Unsaved Changes Protection

- Dirty tracking via JSON snapshot comparison
- Close attempt with unsaved changes triggers `confirm()` dialog
- Discard button resets to original snapshot and clears undo history

### Canvas Error States

- Empty workflow: centered "+" icon with "No activities yet" message
- No workflows: centered "No workflows" with "New Workflow" button
- Node catalog empty search: centered empty-set icon with search query

### Backend Execution Errors (engine.rs)

The workflow engine surfaces these error types that may appear in run history:
- `MaxIterations` — activity exceeded 50 LLM turns
- `BudgetExceeded` — token budget limit hit
- `ActivityFailed` — LLM or tool error
- `Exited` — agent decided to stop early (not an error)
- `Cancelled` — user cancellation
- `CircuitBreak` — 3+ consecutive same-pattern failures

---

## File Reference

| File | Responsibility |
|------|----------------|
| `app/src/lib/components/workflow/WorkflowBuilder.svelte` | Top-level builder: state, mutations, undo/redo, validation, keyboard shortcuts |
| `app/src/lib/components/workflow/BuilderCanvas.svelte` | Canvas rendering: SVG edges, HTML nodes, pan/zoom, drag, context menus |
| `app/src/lib/components/workflow/BuilderChat.svelte` | AI architect chat panel (simulated) |
| `app/src/lib/components/workflow/NodeCatalog.svelte` | Slide-in node type catalog with search and drag support |
| `app/src/lib/components/workflow/NodeConfigPanel.svelte` | Right panel: trigger config, activity detail, steps editor |
| `app/src/lib/components/WorkflowCanvas.svelte` | Read-only workflow canvas (runs/schedule views) |
| `app/src/lib/utils/workflowLayout.ts` | Dagre layout engine, edge paths, mutation helpers |
| `app/src/lib/utils/workflowTypes.ts` | Activity type system: definitions, defaults, catalog mapping |
| `app/src/lib/types/agentPage.ts` | TypeScript interfaces: WorkflowConfig, WorkflowActivity, WorkflowTrigger |
| `app/src/lib/tokens.ts` | Static catalog items, architect intro message |
| `app/src/routes/[agentId]/+layout.svelte` | Modal host: opens builder, passes workflows, handles save |
| `crates/workflow/src/engine.rs` | Backend workflow executor: activity loop, LLM calls, tool execution |
| `crates/workflow/src/parser.rs` | Workflow JSON parser + validator |
| `crates/workflow/src/triggers.rs` | Trigger registration: cron jobs, event subscriptions |
| `crates/workflow/src/events.rs` | Event dispatcher: pattern matching, workflow triggering |
| `crates/workflow/src/lib.rs` | Crate root: error types, public exports |
