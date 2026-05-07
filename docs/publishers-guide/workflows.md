# Workflows & Automation

> Part of the [Agent](agents.md) package. Workflows are defined in the `workflows` map inside `agent.json`. An agent without workflows is valid — it provides only a persona and skills for interactive chat.

Workflows are the automation engine. Each workflow binding pairs a **trigger** (when to run) with **activities** (what to do). The Agent owns the schedule; the workflow owns the procedure.

> **Key principle:** The workflow doesn't decide when it runs. The Agent does. The same procedure could run at 7am in one Agent and 9am in another.

---

## Workflow Binding

Each entry in the `workflows` map binds activities to a trigger:

```json
{
  "workflows": {
    "morning-briefing": {
      "trigger": { "type": "schedule", "cron": "0 7 * * *" },
      "description": "Daily morning briefing",
      "activities": [
        {
          "id": "gather",
          "intent": "Gather today's priorities from calendar and email",
          "skills": ["@nebo/skills/briefing-writer"],
          "steps": ["Check calendar for today", "Scan inbox for urgent items", "Compose briefing"],
          "token_budget": { "max": 4096 }
        },
        {
          "id": "deliver",
          "intent": "Send the briefing to the user",
          "steps": ["Format as concise bullet points", "Post to chat"],
          "token_budget": { "max": 1024 }
        }
      ],
      "budget": { "total_per_run": 6000 },
      "emit": "briefing.ready"
    }
  }
}
```

### Binding Fields

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `ref` | string | no | — | External workflow qualified name (`@org/workflows/name@version`). Optional when using inline `activities`. |
| `trigger` | object | yes | — | When this workflow runs (see [Trigger Types](#trigger-types)) |
| `description` | string | no | `""` | Human-readable description |
| `inputs` | map | no | `{}` | Default inputs passed to the workflow on trigger |
| `activities` | array | no | `[]` | Inline activity definitions. When present, the workflow runs inline — no external `ref` needed. |
| `budget` | object | no | `{}` | Token budget constraints (`total_per_run`) |
| `emit` | string | no | — | Event name to emit on completion. Emitted as `{agent-slug}.{emit}` into the EventBus. |

---

## Activities

Activities define the steps an agent takes when a workflow fires. Each activity is an autonomous LLM task with full tool access. Activities execute **sequentially** — each activity's output becomes context for the next.

### Activity Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | yes | Unique activity ID within the binding |
| `intent` | string | yes | Task description — what the LLM should accomplish |
| `steps` | string[] | no | Step-by-step hints for the LLM |
| `skills` | string[] | no | Skill references available to this activity (`@org/skills/name`) |
| `mcps` | string[] | no | MCP server slugs available to this activity |
| `model` | string | no | Model override (`"sonnet"`, `"haiku"`, `"opus"`) |
| `token_budget` | object | no | Per-activity token limit: `{ "max": 4096 }` |
| `on_error` | object | no | Error handling: `{ "retry": 1, "fallback": "skip" }` |
| `min_iterations` | number | no | Force LLM to continue even if it wants to stop early |

### How Activities Execute

1. **System prompt** is constructed from: execution rules → skills → available tools → intent → steps → inputs → prior activity results
2. **Agentic loop** runs (up to 50 iterations): LLM streams → tool calls → results → repeat
3. **Context chaining**: Each activity's output is appended as `[Activity '{id}' result]: {text}` and passed to the next activity
4. **Completion**: Activity ends when LLM produces text without tool calls (or budget exhausted)

### Built-in Tools

Two tools are always available inside activities:

| Tool | Purpose |
|------|---------|
| `emit` | Emit an event into the EventBus. Other agents can subscribe to it. |
| `exit` | Stop the workflow early with a reason (clean termination, not failure). |

### Error Handling

| Field | Type | Description |
|-------|------|-------------|
| `on_error.retry` | number | Times to retry the activity before giving up (default: 0) |
| `on_error.fallback` | string | What to do after retries exhausted: `skip`, `abort`, `notify_owner` |

**Circuit breaker:** If 3+ consecutive activities fail with the same error pattern, the workflow stops automatically.

**Same-tool loop detection:** If the LLM calls the same tool 3+ times in a row, a steering hint is injected to break the loop.

---

## Budget Math

When using inline activities with a `budget.total_per_run`, the sum of all activity `token_budget.max` values **must not exceed** `total_per_run`. This is validated at parse time — a mismatch prevents the agent from loading.

```
Activity 1: gather  → 4,096 tokens
Activity 2: deliver → 1,024 tokens
                       ─────
Sum:                   5,120 tokens
budget.total_per_run:  6,000 tokens  ✓ (>= sum, headroom for retries)
```

Both per-activity and global budgets are enforced independently at runtime.

---

## Trigger Types

| Type | Fields | Description |
|------|--------|-------------|
| `schedule` | `cron` | Fires on a cron schedule (standard 5-field expression) |
| `heartbeat` | `interval`, `window` (optional) | Fires at a recurring interval, optionally limited to a time window |
| `event` | `sources` | Fires when a matching event occurs |
| `watch` | `plugin`, `event`, `command`, `restart_delay_secs` | Long-running plugin process that emits NDJSON events |
| `manual` | — | Only fires by explicit user request or API call |

### Schedule

```json
{ "type": "schedule", "cron": "0 7 * * 1-5" }
```

Standard 5-field cron. Evaluated every 60 seconds against the agent's configured timezone.

### Heartbeat

```json
{ "type": "heartbeat", "interval": "30m", "window": "08:00-18:00" }
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `interval` | string | yes | Duration string: `"30m"`, `"1h"`, `"2h30m"` |
| `window` | string | no | Active hours (`"HH:MM-HH:MM"`). Outside the window, heartbeats are skipped. |

### Event

```json
{ "type": "event", "sources": ["email.urgent", "calendar.*"] }
```

| Field | Type | Description |
|-------|------|-------------|
| `sources` | string[] | Event source patterns to match. Supports exact (`email.urgent`) and wildcard (`email.*`). |

An empty `sources` array is valid JSON but the trigger will never fire — always include at least one source.

### Watch

```json
{
  "type": "watch",
  "plugin": "gws",
  "event": "email.new",
  "command": "gmail +watch --format ndjson",
  "restart_delay_secs": 5
}
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `plugin` | string | yes | — | Plugin slug |
| `event` | string | no | — | Plugin event name. Enables auto-emission into EventBus. |
| `command` | string | no | `""` | CLI args appended to plugin binary. Required if `event` is not set. |
| `restart_delay_secs` | u64 | no | `5` | Seconds to wait before restarting on crash |

**How it works:**
1. Spawns `<plugin-binary> <command>` as a long-running subprocess
2. Plugin outputs NDJSON (one JSON object per line) to stdout
3. Each line triggers the bound activities with the parsed JSON as `_watch_payload` input
4. If `event` is set, each line also auto-emits into the EventBus as `{plugin}.{event}`
5. On crash, restarts after `restart_delay_secs` with exponential backoff (max 300s)

> **Always provide a `command` fallback** alongside `event`. If the plugin manifest doesn't declare the event, resolution fails silently. `command` ensures the watcher starts regardless.

### Template Substitution in Commands

Watch trigger commands support `{{key}}` placeholders, substituted from the agent's input values at runtime:

```json
"command": "gmail +watch --format ndjson --project {{gcp_project}}"
```

The placeholder name must exactly match an input `key`. Unmatched placeholders are left as literal text (which will cause failures).

### Manual

```json
{ "type": "manual" }
```

Only fires via API call (`POST /agents/{id}/workflows/{binding}/run`) or explicit user request.

---

## Event System

Event triggers let workflows react to things that happen — emails arriving, workflows completing, platform changes detected.

### Event Sources

| Source | Mechanism | Example values |
|--------|-----------|----------------|
| **emit** | Activity calls `emit` tool | `email.customer-service`, `lead.qualified` |
| **watch** | Watch trigger auto-emits | `gws.email.new`, `gws.calendar.changed` |
| **platform** | Platform capabilities | `calendar.changed`, `email.received` |
| **system** | Nebo lifecycle | `workflow.{id}.completed`, `workflow.{id}.failed` |

### Event Envelope

```json
{
  "source": "email.customer-service",
  "payload": { "from": "j@example.com", "subject": "Order issue" },
  "origin": "workflow:email-triage:run-550e8400",
  "timestamp": 1709740800
}
```

The `payload` becomes the triggered workflow's inputs (available as `_event_payload`).

### Source Matching

| Pattern | Matches |
|---------|---------|
| `email.urgent` | Exact match only |
| `email.*` | Any event starting with `email.` |

### Emit Namespace

When a workflow emits events, they are namespaced by the agent's slugified name:

```
Agent name: "Chief of Staff"
emit: "briefing.ready"
→ Event source: "chief-of-staff.briefing.ready"
```

Other agents subscribe to the full namespaced source.

### Emitting from Activities

```
emit(source: "email.customer-service", payload: {"from": "j@example.com", "subject": "..."})
```

Each `emit` call is a discrete event. If an activity emits 5 events, each fires independently against all active subscriptions. This enables **fan-out pipelines**.

### System Events

| Event | When |
|-------|------|
| `workflow.{id}.completed` | A workflow run finishes successfully |
| `workflow.{id}.failed` | A workflow run fails |

System events enable workflow chaining: A completes → B triggers.

---

## Event-Only Watches

A watch with `event` set but no activities is valid. It relays plugin output into the EventBus without processing anything inline:

```json
{
  "email-relay": {
    "trigger": {
      "type": "watch",
      "plugin": "gws",
      "event": "email.new",
      "command": "gmail +watch --format ndjson"
    },
    "description": "Relay new email events into the EventBus"
  }
}
```

Other agents subscribe:

```json
{
  "handle-emails": {
    "trigger": { "type": "event", "sources": ["gws.email.new"] },
    "activities": [...]
  }
}
```

---

## Example: Email Triage Pipeline

```json
{
  "workflows": {
    "email-watcher": {
      "trigger": {
        "type": "watch",
        "plugin": "gws",
        "event": "email.new",
        "command": "gmail +watch --format ndjson"
      },
      "description": "React to new emails in real-time",
      "activities": [
        {
          "id": "triage",
          "intent": "Classify the incoming email and route it",
          "steps": [
            "Read the email content from _watch_payload",
            "Classify as: customer-service, sales-inquiry, or ignore",
            "If not ignore, emit the appropriate event with email data"
          ],
          "token_budget": { "max": 2048 }
        }
      ],
      "budget": { "total_per_run": 2048 }
    },
    "handle-cs": {
      "trigger": { "type": "event", "sources": ["email.customer-service"] },
      "description": "Handle customer service emails",
      "activities": [
        {
          "id": "respond",
          "intent": "Draft a helpful response to the customer service email",
          "skills": ["@acme/skills/cs-templates"],
          "token_budget": { "max": 3000 }
        }
      ],
      "budget": { "total_per_run": 3000 }
    },
    "handle-sales": {
      "trigger": { "type": "event", "sources": ["email.sales-inquiry"] },
      "description": "Handle inbound sales inquiries",
      "activities": [
        {
          "id": "qualify",
          "intent": "Qualify the lead and add to CRM",
          "skills": ["@acme/skills/lead-qualification"],
          "token_budget": { "max": 3000 }
        }
      ],
      "budget": { "total_per_run": 3000 }
    }
  }
}
```

Read top to bottom: watcher monitors Gmail. For each email, triage classifies and emits. Each emission triggers the appropriate handler with the email data as inputs.

---

## Validation Rules

- Each binding must have a `trigger` (required)
- Either `ref` or `activities` must be present (one defines what runs)
- Trigger `type` must be one of: `schedule`, `heartbeat`, `event`, `watch`, `manual`
- Schedule triggers must have a valid 5-field `cron` expression
- Heartbeat triggers must have a valid `interval` (e.g., `"30m"`, `"1h"`)
- Event triggers should have at least one entry in `sources`
- Watch triggers must have a `plugin` and either `event` or `command`
- Activity IDs must be unique within each binding
- If `budget.total_per_run > 0`, the sum of all activity `token_budget.max` must not exceed it
- All `{{key}}` placeholders in commands must match an input `key` exactly
- An Agent with no workflows is valid (chat-only + persona + skills)
