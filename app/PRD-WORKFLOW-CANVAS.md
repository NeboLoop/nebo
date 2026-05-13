# PRD: Workflow Canvas Builder

## Product Vision

An AI-native visual workflow builder that fuses n8n-style automation design with intelligent agent orchestration. Users visually compose, chain, and manage automated workflows while an embedded AI Architect agent assists with design decisions, suggests optimizations, and executes modifications through natural language.

**Nothing like this exists in the desktop AI companion market.** Existing tools force a choice: either you get a powerful visual automation builder (n8n, Zapier, Make) with no AI backbone, or you get an AI companion (ChatGPT, Claude) with no visual automation layer. Nebo's Workflow Canvas sits in the gap — agents that can see, build, and reason about their own automation pipelines.

## Problem Statement

AI companion users today have no way to visually compose and manage agent automations. They're limited to:
- Chat-based instructions that agents forget between sessions
- Manual cron jobs and scripts with no visual feedback
- Disconnected tools (calendar, email, code) with no orchestration layer

Power users who discover workflow automation (n8n, Zapier) find those tools have no AI reasoning, no agent delegation, and no natural language interface for building workflows.

## Target Users

1. **Solo operators** — Founders, freelancers who want their AI agents to run recurring processes (morning briefs, content pipelines, lead nurturing) without manual prompting every time.
2. **Small teams** — 2-10 person teams that need multiple agents coordinating across functions (research, marketing, ops) with visual traceability.
3. **Power users** — Technical users who currently use n8n/Zapier and want AI-native automation with agent delegation, natural language configuration, and intelligent error recovery.

## Core Concepts

### Workflow
A named automation consisting of a trigger, one or more activities executed in sequence, and an optional emit event. Workflows belong to an agent and can chain to other agent workflows via events.

### Activity
A unit of work within a workflow. Activities have an intent (what to do), skills (tools to use), and steps (ordered instructions). Activity types include: Custom, Research, Email, Notify, Code, Condition, and Agent Delegation.

### Trigger
What starts a workflow. Four types:
- **Schedule** — Run at specific times (e.g., "8:00 AM daily")
- **Heartbeat** — Run on interval (e.g., every 30 minutes)
- **Event** — React to a named event emitted by another workflow
- **Manual** — Triggered on demand by user or agent

### Emit
An optional event announced when a workflow completes. Other workflows can listen for this event as their trigger, enabling workflow chaining.

### Agent Delegation
A special activity type where work is handed to a different agent. The delegating workflow pauses (or continues) while the other agent performs the requested task.

### Architect
A purpose-built AI assistant embedded in the canvas that helps users design, modify, and troubleshoot workflows through natural language. Not one of the user's installed agents — a dedicated builder agent with deep knowledge of the workflow system.

## User Experience

### Layout

```
+------------------------------------------------------------------+
| Agent Name - Workflow Builder           [View | Edit]    [x]     |
+------------+------------------------------+----------------------+
| ARCHITECT  | TOOLBAR                      |                      |
| CHAT       | [Architect] [New] [+Node]    |                      |
|            | [Tidy] | ---------- [V|E]    |                      |
| 320px      +------------------------------+ NODE CONFIG          |
| collapsible| CANVAS                       | 340px                |
|            |                              | collapsible          |
|            | [trigger]--+--[activity]--+   |                      |
|            |            |             |   | Trigger type         |
|            |     [activity]--[emit]   |   | Schedule config      |
|            |                          |   | Activities list      |
|            | dot grid, pan/zoom       |   | Steps editor         |
|            | bezier edges             |   | Skills tags          |
+------------+------------------------------+----------------------+
```

### Three-Panel Architecture

**Left — Architect Chat (collapsible)**
AI assistant that understands the workflow graph. Users describe what they want in natural language; the Architect modifies the canvas in response. Shows thinking blocks, tool calls, and confirmation messages.

**Center — Canvas**
Interactive pan/zoom workspace with dot grid. Workflows render as horizontal chains: trigger -> activities -> emit. Bezier curve edges connect nodes. Dashed accent-colored lines show cross-workflow chains (emit -> event trigger). Visible "+" buttons between and after nodes for adding new activities.

**Right — Node Config Panel (collapsible)**
Context-sensitive property editor. Shows different forms depending on what's selected:
- Trigger selected: type picker (4-button grid), schedule/interval/event inputs
- Activity selected: name, intent, skills (tag chips), steps (inline editable list)
- Emit selected: event name input
- No selection: workflow overview with trigger, description, emit, last fired, activity list

### Interactions

| Action | Mechanism | Result |
|--------|-----------|--------|
| Add node | Click "+" between nodes, toolbar "Add Node", or Architect chat | Node Catalog slides in; click item to insert |
| Select node | Click node on canvas | Config panel opens with editable fields |
| Delete node | Right-click > Delete, or select + Delete key | Node removed, edges reconnected |
| Duplicate node | Right-click > Duplicate | Copy inserted after original |
| Configure node | Click node, edit in config panel | Changes reflected on canvas in real-time |
| Create workflow | Toolbar "New Workflow" | Empty workflow with manual trigger appears |
| Switch mode | View/Edit toggle | Edit: full builder. View: read-only inspection |
| Pan canvas | Click + drag on empty space | Canvas moves |
| Zoom | Mouse wheel or +/- buttons | Zoom in/out centered on cursor |
| Fit to screen | Fit button (top-right) | Zoom/pan to show all workflows |
| Tidy up | Toolbar "Tidy Up" | Re-runs auto-layout algorithm |
| Ask Architect | Type in chat: "add an email step" | AI processes request, modifies canvas |

### Node Catalog

Slide-in panel with searchable categorized list:

**Triggers** — Schedule, Heartbeat, Event, Manual
**Activities** — Custom, Research, Email, Notify, Code, Condition
**Agents** — All installed agents (for delegation)
**Output** — Emit Event

## Feature Tiers

### Tier 1 — Foundation (Current + MVP)

**Status: Partially built. Remaining work marked with [TODO].**

- [x] Three-panel layout (chat | canvas | config)
- [x] Pan/zoom canvas with dot grid, bezier edges
- [x] Node rendering with status colors (success/failed/running/idle)
- [x] Add/delete/duplicate nodes via catalog, context menu, keyboard
- [x] Trigger configuration (4 types with conditional inputs)
- [x] Activity editing (name, intent, skills, steps)
- [x] Emit event configuration
- [x] Cross-workflow chain visualization (dashed edges)
- [x] Node Catalog with search and categories
- [x] AI Architect chat (mock responses)
- [x] View/Edit mode toggle
- [x] New Workflow creation
- [x] "+" connector buttons between/after nodes
- [x] Right-click context menu
- [x] Keyboard shortcuts (Delete, Escape)
- [TODO] Save/discard changes (onsave callback + confirmation)
- [TODO] Undo/redo (snapshot stack, Cmd+Z / Cmd+Shift+Z)
- [TODO] Basic validation (required fields, duplicate IDs, orphan nodes)
- [TODO] Error boundaries (graceful layout failure handling)

### Tier 2 — Activity Type System

Each activity type gets a specialized config UI and schema.

- [TODO] **Email** — To, CC, Subject, Body template, Attachments flag
- [TODO] **Code** — Language selector, code editor (monaco or textarea), timeout, environment variables
- [TODO] **Research** — Query template, source selector (web, news, academic), max results, output format
- [TODO] **Notify** — Channel picker (Slack, webhook, push), message template, urgency level
- [TODO] **Condition** — Expression editor, true/false output handles, visual branch paths
- [TODO] **Agent Delegation** — Agent selector, delegation prompt, persona override, wait-for-response toggle, expected output schema

### Tier 3 — Data Flow & Branching

- [TODO] **Typed activity outputs** — Each activity declares its output shape (e.g., `{ emails: Email[], count: number }`)
- [TODO] **Variable picker** — Reference previous activity outputs via `{{step.output.field}}` syntax
- [TODO] **Condition branching** — True/false paths from condition nodes; layout engine supports fork/merge
- [TODO] **Parallel execution** — Fan-out from one node to multiple; fan-in to wait for all
- [TODO] **Error handlers** — Per-activity "on error" configuration (retry, skip, notify, fallback path)

### Tier 4 — Real AI Integration

- [TODO] **Architect -> Claude API** — Replace mock pattern matching with Claude function calling. Define tools: `modify_workflow`, `add_node`, `update_trigger`, `remove_node`, `explain_workflow`
- [TODO] **Architect context** — Pass current workflow graph as structured context to Claude so it can reason about the full pipeline
- [TODO] **Architect suggestions** — Proactive optimization suggestions ("This workflow runs 3 API calls sequentially — want me to parallelize them?")
- [TODO] **Architect history** — Persist chat per workflow for audit trail
- [TODO] **Natural language triggers** — "Run this when I get an email from a VIP" -> auto-configure event trigger

### Tier 5 — Execution & Monitoring

- [TODO] **Live execution overlay** — WebSocket-driven per-node status during real runs (green pulse -> done, red -> error, yellow spinner -> running)
- [TODO] **Execution history** — Click a node to see its last N runs with input/output data
- [TODO] **Step-through debugging** — Pause workflow at a node, inspect state, resume or skip
- [TODO] **Run logs** — Per-execution log with timestamps, token usage, duration, errors
- [TODO] **Test mode** — Dry-run with mock inputs to validate workflow before going live

### Tier 6 — Ecosystem

- [TODO] **Workflow templates** — Save/load workflow patterns from marketplace
- [TODO] **Nested workflows** — Collapse sub-chains into a "group" node; drill down to edit
- [TODO] **Export/Import** — JSON/YAML export; drag-drop import; share between agents
- [TODO] **Version history** — Diff view between saves; rollback to previous version
- [TODO] **Skill catalog integration** — Searchable installed skills picker (replacing freetext input)
- [TODO] **Drag-to-connect** — Drag from node output handle to another node's input to create edge
- [TODO] **Drag-to-reorder** — Drag nodes to reposition; layout engine respects manual positions

## Technical Architecture

### Component Tree

```
+page.svelte (modal host)
  └── WorkflowBuilder.svelte (orchestrator, state management)
        ├── BuilderChat.svelte (wraps ChatPane for Architect)
        ├── BuilderCanvas.svelte (pan/zoom, nodes, edges, "+")
        ├── NodeCatalog.svelte (searchable node picker)
        └── NodeConfigPanel.svelte (editable property forms)
```

### Key Files

| File | Purpose |
|------|---------|
| `src/lib/components/workflow/WorkflowBuilder.svelte` | Top-level orchestrator. Mutable state, mutations, panel coordination |
| `src/lib/components/workflow/BuilderCanvas.svelte` | Canvas with pan/zoom, node rendering, edge SVG, "+" buttons, context menu |
| `src/lib/components/workflow/NodeConfigPanel.svelte` | Right panel with editable forms per node type |
| `src/lib/components/workflow/NodeCatalog.svelte` | Slide-in node type picker with search |
| `src/lib/components/workflow/BuilderChat.svelte` | Thin wrapper around ChatPane for AI Architect |
| `src/lib/utils/workflowLayout.ts` | Layout engine: horizontal chain positioning, bezier paths, mutation helpers |
| `src/lib/components/WorkflowCanvas.svelte` | Original read-only viewer (kept as reference/fallback) |
| `src/lib/mockData.ts` | NODE_CATALOG_ITEMS, ARCHITECT_INTRO_MESSAGE, workflow mock data |

### State Management

All builder state lives as `$state()` runes in `WorkflowBuilder.svelte`:
- `builderWorkflows` — Deep clone of agent's workflows (mutable working copy)
- `selectedNodeId` / `selectedWorkflowName` — Current selection
- `mode` — `'view' | 'edit'`
- `catalogOpen` / `catalogInsertAfter` / `catalogInsertWorkflow` — Catalog state
- `chatOpen` — Architect panel visibility

All mutations go through `updateWorkflow()` which deep-clones before mutating, then assigns a new object to trigger Svelte 5 reactivity.

### Layout Engine

`workflowLayout.ts` positions nodes in a horizontal left-to-right chain:
- Trigger node (160x60) at left
- Activity nodes (220x80) spaced with 60px gaps
- Emit node (160x60) at right
- Bezier curves connect right edge of each node to left edge of next
- Multiple workflows stack vertically with 60px gap between
- Cross-workflow chains: dashed bezier from emit to matching event trigger

### Data Model

```typescript
interface Workflow {
  trigger: { type: 'schedule' | 'heartbeat' | 'event' | 'manual'; schedule?: string; event?: string; interval?: string };
  description: string;
  isActive: boolean;
  lastFired?: string;
  emit?: string;
  activities: Activity[];
}

interface Activity {
  id: string;
  intent: string;
  skills?: string[];
  steps?: string[];
  // Future: type-specific fields
  agentId?: string;      // for delegation
  agentColor?: string;   // for delegation
}
```

## Success Metrics

1. **Engagement** — % of users who open the canvas builder at least once per week
2. **Completion** — % of workflows created in the builder that get saved (vs abandoned)
3. **Architect usage** — % of workflow modifications made via AI chat vs manual editing
4. **Chaining depth** — Average number of workflows chained together via emit/event
5. **Agent delegation** — % of workflows that include delegation to another agent
6. **Time to first workflow** — Minutes from opening builder to saving first workflow

## Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Layout engine can't handle branching | Condition nodes are useless without fork support | Start with sequential-only; add fork layout as Tier 3 |
| Architect gives bad suggestions | User trust erodes | Architect only suggests, never auto-saves. All changes require user confirmation |
| Canvas performance with many nodes | Laggy UX discourages use | Virtual rendering (only render visible nodes); limit workflows per canvas |
| Users don't discover the builder | Feature goes unused | Add "Canvas" button prominently in workflow section; onboarding tooltip |
| Workflow complexity exceeds what agents can execute | Broken expectations | Validation layer warns about unsupported patterns before save |

## Open Questions

1. Should the Architect be able to execute workflows (dry-run) to test them, or just build them?
2. Should workflows be sharable between agents, or strictly agent-scoped?
3. Should we support importing n8n workflow JSON directly?
4. How should the canvas handle 20+ workflows for a single agent — pagination, filtering, or search?
5. Should condition branches support more than two paths (switch/case)?
