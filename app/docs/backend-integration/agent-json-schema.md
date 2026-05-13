# Agent.json Schema — V2 Frontend Requirements

This documents the agent.json schema as understood by the V2 frontend,
including all workflow node types and activity definitions.

---

## Root Schema

```typescript
interface AgentConfig {
  // System prompt — read from AGENT.md or stored in config
  persona?: string;

  // LLM model override
  model?: string;  // e.g., "claude-opus-4-6", "claude-sonnet-4-6"

  // User-configurable input fields (rendered as form in agent settings)
  inputs: AgentInput[];

  // Workflow definitions indexed by workflow name
  workflows: Record<string, WorkflowDefinition>;

  // Skills used by this agent
  skills?: string[];  // e.g., ["@nebo/skills/deep-research@^1.0.0"]

  // Pricing model (marketplace agents)
  pricing?: {
    model: "monthly_fixed" | "free" | string;
    cost: number;
  };

  // Agent defaults
  defaults?: {
    timezone?: "user_local" | string;
    configurable?: string[];  // Paths to user-configurable fields
  };

  // Required dependencies
  requires?: {
    plugins?: string[];  // Plugin IDs needed (e.g., "PLUG-PJ3Z-ECFV")
  };
}
```

## Input Field Schema

```typescript
interface AgentInput {
  key: string;                    // Machine-readable key
  label?: string;                 // Display label
  description?: string;           // Help text shown below input
  type: "text" | "textarea" | "select" | "file" | "path";
  required?: boolean;
  placeholder?: string;
  default?: string;
  options?: Array<{               // Only for type: "select"
    value: string;
    label: string;
  }>;
}
```

**Frontend renders these in:** `[agentId]/settings/[section]/+page.svelte` (configure section)

---

## Workflow Definition Schema

```typescript
interface WorkflowDefinition {
  // Trigger — how the workflow starts
  trigger: WorkflowTrigger;

  // Human-readable description
  description?: string;

  // Whether workflow is currently active
  isActive?: boolean;

  // Timestamp of last execution
  lastFired?: string;

  // Event emitted on completion (other workflows can listen)
  emit?: string;

  // Budget constraint
  budget?: {
    total_per_run: number;
  };

  // Ordered list of activities (nodes in the workflow graph)
  activities?: WorkflowActivity[];

  // Edges connecting activities (for the visual builder)
  connections?: WorkflowConnection[];

  // Workflow-level inputs
  inputs?: Record<string, any>;

  // Origin tracking
  source?: "marketplace" | "custom";
}
```

---

## Trigger Types (4 types)

```typescript
type WorkflowTrigger =
  | ScheduleTrigger
  | EventTrigger
  | HeartbeatTrigger
  | ManualTrigger;

interface ScheduleTrigger {
  type: "schedule";
  schedule: string;     // Human-readable: "8:00 AM daily", "Monday 9:00 AM", "3:00 PM weekdays"
  cron?: string;        // Cron expression: "0 8 * * *"
}

interface EventTrigger {
  type: "event";
  event: string;        // Event name: "GitHub PR opened", "New email received"
  sources?: string[];   // Event sources to listen for
  plugin?: string;      // Plugin ID that emits this event
}

interface HeartbeatTrigger {
  type: "heartbeat";
  interval: string;     // Duration: "5m", "15m", "30m", "1h", "24h"
  window?: {
    start: string;      // Time window start: "08:00"
    end: string;        // Time window end: "18:00"
  };
}

interface ManualTrigger {
  type: "manual";
  // No additional fields — user clicks "Run Now"
}
```

**Frontend node types:**
| Trigger | Node Catalog Type | Icon |
|---------|------------------|------|
| Schedule | `trigger-schedule` | ⏱ |
| Event | `trigger-event` | ⚡ |
| Heartbeat | `trigger-heartbeat` | ♥ |
| Manual | `trigger-manual` | ▶ |

---

## Activity Types (12 types)

```typescript
type ActivityType =
  | "custom"
  | "research"
  | "email"
  | "notify"
  | "code"
  | "condition"
  | "loop"
  | "wait"
  | "agent"
  | "connector"
  | "http"
  | "transform";
```

### Activity Object

```typescript
interface WorkflowActivity {
  id: string;                           // Unique ID within workflow
  type?: ActivityType;                  // Activity type (defaults to "custom")
  intent: string;                       // Goal description
  skills?: string[];                    // Skill references: ["@nebo/skills/web-scraper@^1.0.0"]
  steps?: string[];                     // Implementation steps (human-readable)
  params?: Record<string, any>;         // Type-specific parameters (see below)
  model?: string;                       // LLM model override for this step
  token_budget?: { max: number };       // Token limit for this step
  on_error?: {
    retry?: number;                     // Number of retries
    fallback?: "skip" | "notify_owner" | string;
  };
}
```

### Activity Type Details

#### `custom` — Custom Activity
- **Icon:** ◆
- **Description:** Define steps and skills
- **Default skills:** none
- **Parameters:** none
- **Branching:** no

#### `research` — Research
- **Icon:** ⊕
- **Description:** Web search and analysis
- **Default skills:** `@nebo/skills/web-scraper@^1.0.0`
- **Parameters:**
  - `depth`: `"quick"` | `"standard"` | `"deep"`
  - `sources`: string[] (source URLs or names)
- **Branching:** no

#### `email` — Send Email
- **Icon:** ✉
- **Description:** Compose and send email
- **Default skills:** `@nebo/skills/gws-gmail@^1.0.0`
- **Parameters:**
  - `to`: string (recipient)
  - `subject`: string
- **Branching:** no

#### `notify` — Notify
- **Icon:** ⊘
- **Description:** Send notification
- **Default skills:** `@nebo/skills/slack@^1.0.0`
- **Parameters:**
  - `channel`: `"slack"` | `"email"` | `"webhook"`
  - `target`: string (channel name, email, or URL)
- **Branching:** no

#### `code` — Run Code
- **Icon:** ⌘
- **Description:** Execute a code snippet
- **Default skills:** `@nebo/skills/sandbox@^1.0.0`
- **Parameters:**
  - `language`: `"javascript"` | `"python"` | `"typescript"` | `"shell"`
  - `code`: string
- **Branching:** no

#### `condition` — Condition (If/Else)
- **Icon:** ⑂
- **Description:** If/else branching
- **Default skills:** none
- **Parameters:**
  - `expression`: string (JavaScript expression)
  - `mode`: `"expression"` | `"contains"` | `"exists"` | `"regex"`
- **Branching:** YES — outputs `"True"` and `"False"` branches

#### `loop` — Loop
- **Icon:** ↻
- **Description:** Iterate over items
- **Default skills:** none
- **Parameters:**
  - `source`: string (data source expression)
  - `maxIterations`: number
- **Branching:** YES — outputs `"Each item"` and `"Done"` branches

#### `wait` — Wait
- **Icon:** ⏸
- **Description:** Pause or wait for event
- **Default skills:** none
- **Parameters:**
  - `duration`: string (e.g., "5m", "1h")
  - `waitUntil`: string (event name to wait for)
- **Branching:** no

#### `agent` — Delegate to Agent
- **Icon:** ◉
- **Description:** Delegate work to another agent
- **Default skills:** none
- **Parameters:**
  - `agentId`: string (target agent ID)
  - `instructions`: string (what to tell the agent)
- **Branching:** no

#### `connector` — MCP Connector
- **Icon:** ⊞
- **Description:** Call an MCP server tool
- **Default skills:** none
- **Parameters:**
  - `serverId`: string (MCP integration ID)
  - `tool`: string (tool name from the server)
  - `input`: Record<string, any> (tool input parameters)
- **Branching:** no

#### `http` — HTTP Request
- **Icon:** ⇄
- **Description:** Make an API call
- **Default skills:** none
- **Parameters:**
  - `method`: `"GET"` | `"POST"` | `"PUT"` | `"PATCH"` | `"DELETE"`
  - `url`: string
  - `body`: string (JSON body)
  - `headers`: Record<string, string>
- **Branching:** no

#### `transform` — Transform Data
- **Icon:** ⊿
- **Description:** Reshape or filter data
- **Default skills:** none
- **Parameters:**
  - `operation`: `"map"` | `"filter"` | `"reduce"` | `"pick"` | `"template"`
  - `expression`: string (JavaScript expression)
- **Branching:** no

---

## Connection Schema

Edges that wire the workflow graph together.

```typescript
interface WorkflowConnection {
  from: string;       // Source node ID, "__trigger__", or "__emit__"
  to: string;         // Target node ID or "__emit__"
  label?: string;     // Branch label: "True", "False", "Each item", "Done"
}
```

**Special nodes:**
- `__trigger__` — Virtual entry point (the trigger node)
- `__emit__` — Virtual exit point (emits the workflow's event)

**Example:**
```json
{
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

---

## Full Example: Researcher Agent Config

```json
{
  "persona": "You are the Research Analyst. You find, triangulate, and synthesize information.",
  "model": "claude-sonnet-4-6",
  "inputs": [
    {
      "key": "research_topics",
      "label": "Topics to track",
      "type": "textarea",
      "required": false,
      "placeholder": "AI trends in healthcare",
      "description": "Topics for daily digests, one per line"
    },
    {
      "key": "preferred_depth",
      "label": "Default research depth",
      "type": "select",
      "required": false,
      "options": [
        { "value": "quick", "label": "Quick (2-5 min)" },
        { "value": "standard", "label": "Standard (5-10 min)" },
        { "value": "deep", "label": "Deep (10-20 min)" }
      ],
      "default": "standard"
    }
  ],
  "workflows": {
    "morning-scan": {
      "trigger": { "type": "schedule", "schedule": "8:00 AM daily" },
      "description": "Scan configured topics for overnight developments",
      "isActive": true,
      "emit": "research.digest.ready",
      "activities": [
        {
          "id": "scan-sources",
          "type": "research",
          "intent": "Check all configured topic sources",
          "skills": ["@nebo/skills/web-scraper@^1.0.0"],
          "steps": ["Search news for each configured topic", "Check competitor websites"],
          "params": { "depth": "standard" }
        },
        {
          "id": "check-urgency",
          "type": "condition",
          "intent": "Check if urgent findings exist",
          "params": { "expression": "data.findings.some(f => f.urgency === 'high')", "mode": "expression" }
        },
        {
          "id": "urgent-alert",
          "type": "notify",
          "intent": "Send urgent Slack alert",
          "skills": ["@nebo/skills/slack@^1.0.0"],
          "params": { "channel": "slack", "target": "#alerts" }
        },
        {
          "id": "digest",
          "type": "custom",
          "intent": "Compile findings into a digest",
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
    },
    "deep-dive": {
      "trigger": { "type": "manual" },
      "description": "Full research: scope, plan, retrieve, triangulate, synthesize",
      "isActive": true,
      "activities": [
        {
          "id": "scope-and-plan",
          "type": "research",
          "intent": "Define research question and search strategy",
          "skills": ["@nebo/skills/deep-research@^1.0.0"],
          "params": { "depth": "deep" }
        },
        {
          "id": "retrieve-and-triangulate",
          "type": "research",
          "intent": "Gather and cross-reference from multiple sources",
          "skills": ["@nebo/skills/deep-research@^1.0.0", "@nebo/skills/web-scraper@^1.0.0"]
        },
        {
          "id": "synthesize-and-package",
          "type": "custom",
          "intent": "Write the final research report",
          "skills": ["@nebo/skills/deep-research@^1.0.0"]
        }
      ]
    }
  }
}
```
