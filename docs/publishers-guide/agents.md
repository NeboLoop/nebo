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
2. **Watch trigger template substitution** — `{{key}}` placeholders in watch trigger commands are replaced with the corresponding input value at runtime. Example: `gmail +watch --project {{gcp_project}}`. **The placeholder name must exactly match an input `key`** — if the command uses `{{gcp_project}}`, there must be an input with `"key": "gcp_project"`. Unmatched placeholders are left as literal text (e.g., `--project {{gcp_project}}`), which will cause the watch command to fail or behave unexpectedly.
3. **Stored separately from schema** — The input field *schema* lives in `agent.json`. The user-supplied *values* are stored in the `input_values` DB column and updated via `PUT /agents/{id}/inputs`.

### Workflows Overview

The `workflows` map pairs triggers (when to run) with activities (what to do). Each binding connects a trigger to either inline activities or an external workflow reference.

For the full reference — trigger types, activities, event system, watch triggers, budget math, and examples — see **[Workflows & Automation](workflows.md)**.

### Trigger Types (Summary)

| Type | Description |
|------|-------------|
| `schedule` | Fires on a cron schedule |
| `heartbeat` | Fires at a recurring interval (with optional time window) |
| `event` | Fires when a matching event occurs |
| `watch` | Long-running plugin process emitting NDJSON |
| `manual` | Only fires by explicit user request or API call |

---

## views.json & theme.css — Workspace UI (Optional)

Agents can declare deterministic workspace UIs that render immediately — no LLM call required. This is for agents with a known interface: dashboards, control panels, status displays, input forms.

- `views.json` — declares components, data bindings, and actions
- `theme.css` — agent-specific CSS styling for the workspace

Agents without `views.json` are chat-only (or can create UIs dynamically via the `a2ui` tool during conversation).

For the full component catalog, data binding reference, and action types, see **[Workspace Views](views.md)**.

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

- **Added:** New agent directory or symlink with `AGENT.md` → appears in sidebar and Apps page
- **Changed:** Edits to `AGENT.md`, `agent.json`, `manifest.json`, `views.json`, or `theme.css` → metadata updated in DB, worker restarted if active
- **Removed:** Deleted directory → agent soft-deactivated (DB record preserved with `is_enabled=0`)

Changes are debounced at 1 second. No restart needed. Symlinks are fully supported — you can symlink an entire app directory from your source repo into `~/.nebo/user/agents/` and changes take effect immediately.

---

## Validation Rules

- Each workflow binding must have a `trigger`
- Trigger type must be one of: `schedule`, `heartbeat`, `event`, `watch`, `manual`
- Workflow bindings with invalid triggers are skipped with a warning — they do not prevent the agent from loading
- Schedule triggers must have a valid `cron` expression
- Heartbeat triggers must have a valid `interval` (e.g., `"30m"`, `"1h"`)
- Event triggers should have at least one entry in `sources` — an empty array is accepted but the trigger will never fire
- Skill refs must be qualified names (`@org/skills/name`)
- Watch triggers must have a `plugin` and either `event` or `command` (both recommended — `command` as fallback for when `event` resolution fails)
- Activity IDs must be unique within each binding
- If `budget.total_per_run > 0`, the sum of all activity `token_budget.max` values must not exceed it
- All `{{key}}` placeholders in watch trigger commands must match an input `key` exactly
- An Agent with no workflows is valid — it provides only a persona and skill declarations
