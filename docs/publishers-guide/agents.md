# Agents (`@org/agents/name`)

An Agent is a job description with a schedule. It bundles workflows and skills into a complete job profile — and it defines *when* each workflow runs. The Agent is the only artifact type that owns event bindings.

An Agent is three files: `manifest.json` (identity), `agent.json` (operational wiring), and `AGENT.md` (persona). Optionally:
- `views.json` declares deterministic workspace UIs that render immediately without LLM involvement
- `theme.css` provides agent-specific CSS styling for workspace UIs

For packaging format and manifest.json, see [Packaging](packaging.md).

---

## agent.json — The Job Definition

The `agent.json` carries the operational structure: which workflows to run, when to run them, and what events to listen for. This is the file that makes an Agent more than a folder of workflows — it's what makes it an employee who already knows the job.

```json
{
  "workflows": {
    "morning-briefing": {
      "ref": "@nebo/workflows/daily-briefing@^1.0.0",
      "trigger": {
        "type": "schedule",
        "cron": "0 7 * * *"
      },
      "description": "Daily morning briefing before the user wakes up"
    },
    "day-monitor": {
      "ref": "@nebo/workflows/day-monitor@^1.0.0",
      "trigger": {
        "type": "heartbeat",
        "interval": "30m",
        "window": "08:00-18:00"
      },
      "description": "Monitors for changes and interrupts only when something matters"
    },
    "evening-wrap": {
      "ref": "@nebo/workflows/evening-wrap@^1.0.0",
      "trigger": {
        "type": "schedule",
        "cron": "0 18 * * *"
      },
      "description": "End of day summary — what happened, what's unresolved, what's tomorrow"
    },
    "interrupt": {
      "ref": "@nebo/workflows/urgent-interrupt@^1.0.0",
      "trigger": {
        "type": "event",
        "sources": ["calendar.changed", "email.urgent"]
      },
      "description": "Fires when something urgent surfaces that needs immediate attention"
    }
  },
  "requires": {
    "plugins": ["PLUG-PJ3Z-ECFV"]
  },
  "skills": [
    "@nebo/skills/briefing-writer@^1.0.0"
  ],
  "pricing": {
    "model": "monthly_fixed",
    "cost": 47.0
  },
  "inputs": [
    {
      "key": "timezone",
      "label": "Your Timezone",
      "type": "select",
      "required": true,
      "default": "US/Eastern",
      "options": [
        { "value": "US/Eastern", "label": "Eastern" },
        { "value": "US/Pacific", "label": "Pacific" }
      ]
    },
    {
      "key": "briefing_focus",
      "label": "What should briefings focus on?",
      "type": "textarea",
      "placeholder": "e.g., sales pipeline, client deadlines, market news"
    }
  ],
  "defaults": {
    "timezone": "user_local",
    "configurable": ["workflows.morning-briefing.trigger.cron", "workflows.evening-wrap.trigger.cron", "workflows.day-monitor.trigger.interval"]
  }
}
```

---

## agent.json Fields

### Top-Level

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `workflows` | map | no | `{}` | Workflow bindings with triggers (keyed by binding name) |
| `requires` | object | no | `{}` | Hard dependencies. `requires.plugins` is an array of plugin install codes (e.g., `["PLUG-PJ3Z-ECFV"]`) that are auto-installed before skills during agent install. |
| `skills` | string[] | no | `[]` | Additional skill qualified names (beyond what workflows declare) |
| `inputs` | array | no | `[]` | Input field definitions for the agent's Configure tab (see [Input Fields](#input-fields) below) |
| `pricing` | object | no | — | Pricing configuration (see below) |
| `defaults` | object | no | `{}` | Default settings and user-configurable fields (see below) |

### Pricing

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `model` | string | yes | Pricing model: `monthly_fixed` or `per_run` |
| `cost` | float | yes | Price in USD. For `monthly_fixed`, the monthly subscription price. For `per_run`, the cost per workflow execution. |

### Defaults

| Field | Type | Description |
|-------|------|-------------|
| `timezone` | string | Timezone for schedule triggers. `user_local` resolves to the user's system timezone at install time. Also accepts IANA timezone names (e.g., `America/New_York`). |
| `configurable` | string[] | JSON paths within `agent.json` that the user can override after installation. |

### Input Fields

Input fields define a dynamic form rendered in the agent's Configure tab. Users fill in values after installation; the agent uses them at runtime.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `key` | string | yes | Unique reference key (used in `{{key}}` template substitution and system prompt injection) |
| `label` | string | yes | Display label shown to the user |
| `type` | string | yes | Field type: `text`, `textarea`, `number`, `select`, `checkbox`, `radio` |
| `description` | string | no | Help text displayed below the field |
| `required` | boolean | no | Whether the field must be filled before the agent can activate |
| `default` | any | no | Default value pre-filled in the form |
| `placeholder` | string | no | Placeholder text for text/textarea fields |
| `options` | array | no | For `select`/`radio` fields: `[{ "value": "...", "label": "..." }]` |

**How input values are used:**

1. **System prompt injection** — All filled input values are appended to the agent's system prompt as a "Configured Inputs" section. The LLM sees them and uses them without asking the user again.
2. **Watch trigger template substitution** — `{{key}}` placeholders in watch trigger commands are replaced with the corresponding input value at runtime. Example: `gmail +watch --project {{gcp_project}}`.
3. **Stored separately from schema** — The input field *schema* lives in `agent.json`. The user-supplied *values* are stored in the `input_values` DB column and updated via `PUT /agents/{id}/inputs`.

### Inline Activities

Activities define the steps an agent takes when a workflow binding fires. Each activity is an LLM task with full tool access.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | yes | Unique activity ID within the binding |
| `intent` | string | yes | Task description — what the LLM should accomplish |
| `steps` | string[] | no | Numbered step hints for the LLM |
| `skills` | string[] | no | Skill references available to this activity |
| `mcps` | string[] | no | MCP server references available to this activity |
| `model` | string | no | Model override (e.g., `"sonnet"`, `"haiku"`) |
| `token_budget` | object | no | Per-activity token limit (`{ "max": 4096 }`) |
| `on_error` | object | no | Error handling: `{ "retry": 1, "fallback": "skip" }`. Fallback options: `skip`, `abort`, `notify_owner` |

Activities execute sequentially. Each activity's output is passed as context to the next. If an activity emits events via the `emit` tool, those events trigger matching subscriptions independently.

### Workflow Binding

Each entry in the `workflows` map binds a workflow to a trigger:

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `ref` | string | no | — | Workflow qualified name (`@org/workflows/name@version`). Optional when using inline `activities`. |
| `trigger` | object | yes | — | When this workflow runs |
| `description` | string | no | `""` | Human-readable description of this binding |
| `inputs` | map | no | `{}` | Default inputs passed to the workflow on trigger |
| `activities` | array | no | `[]` | Inline activity definitions (see below). When present, the workflow runs inline — no external `ref` needed. |
| `budget` | object | no | `{}` | Token budget constraints. `total_per_run` limits total tokens across all activities. |
| `emit` | string | no | — | Event name to emit on workflow completion. Emitted as `{agent_slug}.{emit}` into the EventBus. |

### Trigger Types

| Type | Fields | Description |
|------|--------|-------------|
| `schedule` | `cron` (string) | Fires on a cron schedule. Standard 5-field cron expression. |
| `heartbeat` | `interval` (string), `window` (string, optional) | Fires at a recurring interval. Window limits active hours (e.g., `"08:00-18:00"`). |
| `event` | `sources` (string[]) | Fires when a matching event occurs. See [Event System](#event-system) below. |
| `watch` | `plugin` (string), `event` (string, optional), `command` (string, optional), `restart_delay_secs` (u64, optional) | Long-running plugin process that emits NDJSON events. See [Watch Triggers](#watch-triggers) below. |
| `manual` | — | Only fires by explicit user request or API call. |

> **Key principle:** The workflow doesn't decide when it runs. The Agent does. The same `@acme/workflows/lead-qualification` workflow could run at 7am in one Agent and 9am in another. The procedure doesn't change just because you want your briefing at a different time.

---

## Event System

Event triggers let workflows react to things that happen — an email arriving, a workflow completing, a platform capability detecting a change. The `sources` array in a trigger subscription is a filter: when an event's source string matches, the bound workflow fires.

### Event Sources

Events come from three places:

| Source | Mechanism | Example `source` values |
|--------|-----------|------------------------|
| **emit** | A workflow activity calls the built-in `emit` tool during execution | `email.customer-service`, `lead.qualified` |
| **platform** | Platform capabilities emit events for external changes | `calendar.changed`, `email.received` |
| **system** | Nebo emits lifecycle events automatically | `workflow.email-triage.completed`, `workflow.email-triage.failed` |

> **Webhooks** (external HTTP POST → NeboLoop → agent) are planned but not yet available. Because Nebo runs on the user's computer, inbound webhooks require NeboLoop as a relay, which is post-MVP.

### Event Envelope

Every event has the same shape:

```json
{
  "source": "email.customer-service",
  "payload": {
    "from": "j@example.com",
    "subject": "Order issue",
    "body": "..."
  },
  "origin": "workflow:email-triage:run-550e8400",
  "timestamp": 1709740800
}
```

| Field | Type | Description |
|-------|------|-------------|
| `source` | string | Event type string — matched against trigger `sources` |
| `payload` | map | Arbitrary data — becomes the triggered workflow's `inputs` |
| `origin` | string | Traceability — who emitted the event (workflow run ID, tool ID, or `system`) |
| `timestamp` | u64 | Unix epoch seconds |

### Emitting Events from Workflows

Workflow activities can emit events using the built-in `emit` tool. Each `emit` call is a discrete event. If an activity emits 5 events, each one is matched independently against all active trigger subscriptions.

```
emit(source: "email.customer-service", payload: {"from": "j@example.com", "subject": "..."})
```

This enables **fan-out pipelines**: a triage workflow reads a batch of emails, classifies them, and emits one event per classified email. Each event triggers the appropriate handler workflow with that email's data as inputs.

### Platform Events

Platform capabilities emit events for external changes they detect. The `calendar` capability emits events like `calendar.changed`, `calendar.conflict`. The `email` capability emits `email.received`, `email.urgent`.

The event namespace follows the capability name: capability `X` → `X.*` events.

### System Events

Nebo emits lifecycle events automatically:

| Event | When |
|-------|------|
| `workflow.{id}.completed` | A workflow run finishes successfully |
| `workflow.{id}.failed` | A workflow run fails |

System events enable **workflow chaining**: workflow A completes, its completion event triggers workflow B.

### Source Matching

The `sources` array in a trigger subscription supports two matching modes:

| Pattern | Matches |
|---------|---------|
| `email.urgent` | Exact match only |
| `email.*` | Any event starting with `email.` |

When an event is emitted, Nebo checks all active event trigger subscriptions across all installed Agents. Every matching subscription spawns a new workflow run with the event's `payload` as inputs.

### Example: Email Triage Pipeline

```json
{
  "workflows": {
    "email-triage": {
      "ref": "@acme/workflows/email-triage@^1.0.0",
      "trigger": {
        "type": "schedule",
        "cron": "*/30 * * * *"
      },
      "description": "Read inbox, classify emails, route to handlers"
    },
    "handle-cs": {
      "ref": "@acme/workflows/handle-cs-email@^1.0.0",
      "trigger": {
        "type": "event",
        "sources": ["email.customer-service"]
      },
      "description": "Handle customer service emails"
    },
    "handle-sales": {
      "ref": "@acme/workflows/handle-sales-email@^1.0.0",
      "trigger": {
        "type": "event",
        "sources": ["email.sales-inquiry"]
      },
      "description": "Handle inbound sales inquiries"
    }
  }
}
```

Read top to bottom: triage runs every 30 minutes. For each email it classifies, it calls `emit` with the appropriate source (`email.customer-service`, `email.sales-inquiry`). Each emit triggers the matching handler workflow with the email data as inputs.

---

## Watch Triggers

Watch triggers run a long-lived plugin process that outputs NDJSON to stdout. Each JSON line triggers the bound activities and optionally auto-emits into the EventBus so other agents can subscribe.

### With a Plugin Event (Recommended)

Reference an event declared in the plugin's `plugin.json`. The CLI command is resolved from the manifest automatically:

```json
{
  "email-watcher": {
    "trigger": {
      "type": "watch",
      "plugin": "gws",
      "event": "email.new",
      "restart_delay_secs": 5
    },
    "description": "React to new emails in real-time",
    "activities": [
      {
        "id": "triage",
        "intent": "Triage the incoming email",
        "steps": ["Classify urgency", "Draft response if needed"]
      }
    ]
  }
}
```

### With an Explicit Command

Specify the CLI args directly instead of referencing a manifest event:

```json
{
  "trigger": {
    "type": "watch",
    "plugin": "gws",
    "command": "gmail +watch --format ndjson",
    "restart_delay_secs": 5
  }
}
```

### Watch Trigger Fields

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `plugin` | string | Yes | — | Plugin slug (e.g., `"gws"`) |
| `event` | string | No | — | Plugin event name. Resolves command from the plugin manifest's `events` array |
| `command` | string | No | `""` | CLI args appended to the plugin binary. Required if `event` is not set |
| `restart_delay_secs` | u64 | No | `5` | Seconds to wait before restarting the process on crash |

### How It Works

1. The `AgentWorker` spawns `<plugin-binary> <command>` as a long-running subprocess
2. The plugin outputs NDJSON (one JSON object per line) to stdout
3. Each parsed JSON line triggers the bound activities (if any)
4. If `event` is set, each line also auto-emits into the EventBus as `{plugin}.{event}` (e.g., `gws.email.new`)
5. If the process crashes, it restarts after `restart_delay_secs`

### Event-Only Watches

A watch with `event` set but no activities is valid. It auto-emits into the EventBus without processing anything inline. Other agents can subscribe to these events via `event` triggers:

```json
{
  "email-relay": {
    "trigger": {
      "type": "watch",
      "plugin": "gws",
      "event": "email.new"
    },
    "description": "Relay new email events into the EventBus for other agents"
  }
}
```

Another agent subscribes:

```json
{
  "handle-emails": {
    "trigger": {
      "type": "event",
      "sources": ["gws.email.new"]
    },
    "activities": [...]
  }
}
```

### Template Substitution

Event commands from the plugin manifest support `{{key}}` placeholders, substituted from the agent's `input_values` at runtime:

```json
"command": "gmail +watch --format ndjson --project {{gcp_project}}"
```

For more on plugin events and the NDJSON protocol, see [Plugins — Plugin Events](plugins.md#plugin-events).

---

## views.json — Deterministic Workspace UI (Optional)

The `views.json` file declares workspace UIs that render immediately when a user opens the agent's chat — no LLM call required. This is for agents that have a known interface: dashboards, control panels, status displays, input forms.

If an agent has no `views.json`, it can still create workspaces dynamically via the `a2ui` tool during conversation.

```json
{
  "default": {
    "surface_type": "panel",
    "components": [
      { "id": "root", "component": "Column", "children": ["title", "metrics", "scan-btn"] },
      { "id": "title", "component": "Text", "text": "Morning Briefing", "variant": "h2" },
      { "id": "metrics", "component": "Text", "text": { "path": "/summary" } },
      { "id": "scan-btn", "component": "Button", "child": "scan-label", "variant": "primary",
        "action": { "event": { "name": "refresh", "context": {} } } },
      { "id": "scan-label", "component": "Text", "text": "Refresh" }
    ],
    "data": {
      "summary": "Loading..."
    },
    "data_bindings": [
      {
        "path": "/summary",
        "source": { "server": "slack", "tool": "get_daily_summary" },
        "interval_secs": 60
      }
    ],
    "actions": {
      "refresh": {
        "type": "mcp_call",
        "server": "slack",
        "tool": "get_daily_summary",
        "update_path": "/summary"
      }
    }
  },
  "_nav": [
    { "viewId": "default", "label": "Dashboard" },
    { "viewId": "settings", "label": "Settings" }
  ]
}
```

### Structure

The file is a map of **view ID → view definition**. The `default` view renders automatically when the user opens the agent's chat. The special `_nav` key defines workspace navigation tabs.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `surface_type` | string | no | `"panel"` (default). Where the workspace renders. |
| `components` | array | yes | A2UI component tree. Each entry is a component object. |
| `data` | object | no | Initial data model for the view. Values referenced by data bindings in components. |
| `data_bindings` | array | no | Polling definitions — automatically fetch data from MCP tools at intervals (see below). |
| `actions` | map | no | Deterministic action handlers — map action names to handlers that execute without LLM involvement (see below). |

### Component Objects

Each component in the `components` array:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | yes | Unique component ID within the view |
| `component` | string | yes | Component type from the A2UI basic catalog |
| `children` | string[] | no | IDs of child components (for layout components) |
| `text` | string | no | Text content (for Text components) |
| `variant` | string | no | Style variant (`h1`, `h2`, `h3`, `body`, `caption`) |
| `label` | string | no | Button/input label |
| `action` | object | no | Action to fire on interaction (for Button components) |

### Available Components

Components come from the [A2UI v0.9 basic catalog](https://github.com/google/A2UI). All 18 components are supported.

#### Layout

| Component | Description | Key Props |
|-----------|-------------|-----------|
| `Column` | Vertical layout | `children`, `justify`, `align` |
| `Row` | Horizontal layout | `children`, `justify`, `align` |
| `Card` | Container with border/elevation | `child` (single child ID) |
| `List` | Vertical or horizontal list | `children`, `direction`, `align` |
| `Tabs` | Tabbed content sections | `tabs` (array of `{title, child}`) |
| `Modal` | Dialog overlay | `trigger` (component ID), `content` (component ID) |
| `Divider` | Visual separator | `axis` (`horizontal` or `vertical`) |

#### Content

| Component | Description | Key Props |
|-----------|-------------|-----------|
| `Text` | Text display (Markdown supported) | `text`, `variant` (`h1`–`h5`, `body`, `caption`) |
| `Icon` | Material Design icon | `name` (e.g. `check`, `settings`, `search`) |
| `Image` | Image display | `url`, `description`, `fit`, `variant` |
| `AudioPlayer` | Audio playback | `url`, `description` |
| `Video` | Video playback | `url` |

#### Inputs

| Component | Description | Key Props |
|-----------|-------------|-----------|
| `Button` | Clickable action trigger | `child` (Text or Icon ID), `action`, `variant` (`default`, `primary`, `borderless`) |
| `TextField` | Text input | `label`, `value`, `variant` (`shortText`, `longText`, `number`, `obscured`) |
| `Slider` | Numeric range input | `value`, `max`, `min`, `label` |
| `CheckBox` | Boolean toggle | `label`, `value` |
| `ChoicePicker` | Single/multiple selection | `options` (array of `{label, value}`), `value`, `variant` (`mutuallyExclusive`, `multipleSelection`) |
| `DateTimeInput` | Date/time picker | `value` (ISO 8601), `enableDate`, `enableTime`, `label` |

#### Dynamic Values

Props marked as "Dynamic" (`DynamicString`, `DynamicNumber`, `DynamicBoolean`) can be either a literal value or a data binding:

```json
// Literal — static, not user-editable
{ "text": "Hello world" }

// Data binding — reactive, setValue() works
{ "text": { "path": "/data/greeting" } }
```

Use data bindings for any value that should update when the user interacts with input components (Slider, TextField, CheckBox, etc.). Literal values render correctly but `setValue()` is a no-op.

#### Common Props

All components accept these optional props:

| Prop | Type | Description |
|------|------|-------------|
| `weight` | number | Flex-grow weight when inside a Row or Column |
| `accessibility` | object | Accessibility attributes (aria labels, roles) |

### Data Bindings

Data bindings poll MCP tools at regular intervals and inject results into the surface data model. This enables live dashboards without LLM involvement.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `path` | string | yes | JSON Pointer into the data model where results are injected (e.g., `/metrics`) |
| `source.server` | string | yes | MCP server slug (e.g., `"slack"`, `"gws"`) |
| `source.tool` | string | yes | Tool name on the MCP server |
| `params` | object | no | Parameters passed to the tool call |
| `interval_secs` | number | no | Poll interval in seconds (default: 30) |

### Deterministic Actions

Actions declared in `views.json` execute deterministically — no LLM call. Unmatched action names fall through to the LLM for agentic handling.

| Type | Fields | Description |
|------|--------|-------------|
| `mcp_call` | `server`, `tool`, `args`, `update_path` | Calls an MCP tool and injects the result into the data model at `update_path` |
| `navigate` | `view`, `params` | Switches to a different view, optionally passing parameters as initial data |
| `update_data` | `path`, `value` | Directly updates the data model at a JSON Pointer path |

Actions with `type: "agent"` (or no matching type) fall through to the LLM, which processes the action using the full agent context.

### Navigation (`_nav`)

The `_nav` key at the top level of `views.json` defines tabs in the workspace navigation bar.

```json
"_nav": [
  { "viewId": "default", "label": "Dashboard" },
  { "viewId": "settings", "label": "Settings" }
]
```

Each entry has a `viewId` (matching a key in the views map) and a `label` for display.

### Multiple Views

An agent can declare multiple views. Only `default` renders automatically. Other views can be activated by the agent during conversation via the `a2ui` tool or via `navigate` actions:

```json
{
  "default": {
    "components": [...]
  },
  "settings": {
    "components": [...]
  },
  "results": {
    "components": [...]
  }
}
```

### Hot-Reload

During development, changes to `views.json` are picked up by the file watcher with a 1-second debounce. No restart needed.

---

## theme.css — Agent Styling (Optional)

The `theme.css` file provides agent-specific CSS that applies inside the workspace UI. It is loaded dynamically when the user opens the agent's chat and unloaded when they switch away.

```css
/* theme.css */
.btn-primary {
  background-color: #FF6B35;
}
.a2ui-surface-container {
  font-family: 'Inter', sans-serif;
}
```

**How it works:**
- Served via `GET /agents/{id}/theme.css`
- Injected as a `<style data-a2ui-theme>` element with `media="not all"` in `document.head` (prevents global leakage)
- Inside the workspace's shadow DOM, `NeboSurfaceElement` clones the style and enables it (`media=""`)
- A `MutationObserver` watches for dynamically-added stylesheets (supports Vite HMR in dev)

**Hot-reload:** Changes to `theme.css` are picked up by the filesystem watcher alongside `views.json` and `AGENT.md`.

---

## AGENT.md — The Persona

The `AGENT.md` is the agent's job description in prose. It defines who the agent *is* when operating as this Agent — personality, communication style, priorities, judgment calls.

```markdown
# Chief of Staff

You are a Chief of Staff. You have been up for two hours before the
principal opens their eyes. You already know what their day looks like,
what matters most, and what can wait.

Your job is to make sure the principal is never blindsided. You surface
what's important, suppress what isn't, and interrupt only when something
genuinely demands attention.

## Communication Style

- Lead with the one thing that matters most today
- Be direct. No preamble, no pleasantries in briefings
- When you interrupt during the day, say why in one sentence
- Evening wraps are reflective, not just recaps

## Judgment

- "Important" means: time-sensitive, high-stakes, or likely to be missed
- If two things compete for attention, pick the one with a deadline
- Never surface something just because it's new — surface it because it matters
- When in doubt, mention it briefly rather than omit it entirely

## What You Don't Do

- You don't make decisions for the principal — you inform them
- You don't send messages on their behalf unless explicitly told to
- You don't editorialize about their schedule — you present it clearly
```

---

## Auto-Install Cascade

When a user installs an Agent:

1. Parse `agent.json` for `requires.plugins` and skill references
2. Install required plugins from `requires.plugins` (install codes like `PLUG-XXXX-XXXX`)
3. For each workflow binding: install its declared skill dependencies
4. Install any additional skills listed in top-level `skills` array
5. Skills cascade to their own plugin dependencies (via `plugins:` in SKILL.md frontmatter)
6. Register all trigger bindings from `agent.json`
7. Load the AGENT.md persona into the agent's context

The user installs a job. Everything else cascades — plugins first, then skills.

### Filesystem Watcher (Development)

During development, agents placed in `~/.nebo/user/agents/` are detected automatically by the filesystem watcher:

- **Added:** New agent directory with `AGENT.md` → appears in sidebar
- **Changed:** Edits to `AGENT.md`, `agent.json`, `views.json`, or `theme.css` → metadata updated in DB, worker restarted if active
- **Removed:** Deleted directory → agent soft-deactivated (DB record preserved with `is_enabled=0`)

Changes are debounced at 1 second. No restart needed.

---

## Validation Rules

- Each workflow binding must have a valid `ref` (qualified name: `@org/workflows/name@version`) and a `trigger`
- Trigger type must be one of: `schedule`, `heartbeat`, `event`, `watch`, `manual`
- Schedule triggers must have a valid `cron` expression
- Heartbeat triggers must have a valid `interval` (e.g., `"30m"`, `"1h"`)
- Event triggers must have at least one entry in `sources`
- Skill refs must be qualified names (`@org/skills/name`)
- Watch triggers must have a `plugin` and either `event` or `command`
- Activity IDs must be unique within each binding
- An Agent with no workflows is valid — it provides only a persona and skill declarations
