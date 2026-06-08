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
| `trigger` | object | yes | — | When this workflow runs (see [Trigger Types](#trigger-types)) |
| `description` | string | no | `""` | Human-readable description |
| `inputs` | map | no | `{}` | Default inputs passed to the workflow on trigger |
| `activities` | array | yes* | `[]` | Inline activity definitions. Bindings are always inline — there is no external-workflow reference mechanism. (\*Optional only for event-only watches, see below.) |
| `budget` | object | no | `{}` | Token budget constraints (`total_per_run`) |
| `emit` | string | no | — | Event name to emit on completion. By convention, prefix with the agent slug (e.g., `chief-of-staff.briefing.ready`) to avoid cross-agent collisions. |

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
4. **Branch termination**: If an activity produces empty output (empty string), its branch stops — no downstream activities execute. This follows n8n-style semantics where empty output signals "nothing to do."
5. **Completion**: Activity ends when LLM produces text without tool calls (or budget exhausted)

### Activity Results

Each completed activity records:

| Field | Type | Description |
|-------|------|-------------|
| `run_id` | string | The workflow run this result belongs to |
| `activity_id` | string | The activity ID within the binding |
| `status` | string | Activity outcome (e.g., completed, failed) |
| `tokens_used` | number | Combined token usage for the activity |
| `attempts` | number | Number of execution attempts |
| `error` | string | Error message (if failed) |
| `started_at` | timestamp | When the activity began executing |
| `completed_at` | timestamp | When the activity finished |

Activity results are stored per run and available in system events and the workflow run API response.

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

**Circuit breaker** (`CIRCUIT_BREAKER_THRESHOLD = 3`): If 3 or more consecutive activities fail with the same error pattern, the workflow aborts automatically. The failure reason is stored per activity result, enabling post-mortem analysis. This prevents infinite retry loops on systematic errors (e.g., a misconfigured API key causing every activity to fail the same way).

**Same-tool loop detection:** If the LLM calls the same tool 3+ times in a row, a steering hint is injected to break the loop.

### Cancellation

Workflows support graceful cancellation via a cancellation token. When cancelled:

1. The currently in-progress activity is allowed to finish (no mid-activity abort)
2. No subsequent activities are started
3. The workflow run is marked as cancelled, not failed

Cancel a running workflow via `POST /workflows/{id}/runs/{runId}/cancel`.

---

## Budget Math

When using inline activities with a `budget.total_per_run`, the sum of all activity `token_budget.max` values should not exceed `total_per_run`. This sum-vs-total check is enforced at parse time for standalone `workflow.json` files; for inline activity bindings in `agent.json` it is **not** validated at load time, so keep the budgets consistent yourself. Both per-activity and global budgets are enforced independently at runtime regardless.

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
| `folder` | `path`, `extensions`, `recursive`, `debounce_secs` | Fires when files change in a watched directory |
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

### Input Values & Template Substitution in Triggers

`{{key}}` template substitution applies only to the watch trigger `command` and the folder trigger `path`. Values are resolved from `agents.input_values`, which users set via the Settings UI or the API. Schedule/cron and other trigger configs receive the raw value with no substitution.

```json
{
  "trigger": { "type": "folder", "path": "{{watch_dir}}/inbox" }
}
```

If the agent's `input_values` contains `{ "watch_dir": "/Users/me/Documents" }`, the folder path resolves to `/Users/me/Documents/inbox` at runtime. This lets end users customize the watched command or path without editing the agent package.

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
  "origin": "agent-42-run-550e8400",
  "timestamp": 1709740800
}
```

The `payload` becomes the triggered workflow's inputs (available as `_event_payload`). The `origin` format varies by source — activity `emit` calls use the emitting session key (e.g., `agent-{id}-{run_id}`), while folder events use `folder:{binding_name}`.

### Source Matching

| Pattern | Matches |
|---------|---------|
| `email.urgent` | Exact match only |
| `email.*` | Any event starting with `email.` |

### Emit Namespace

Events emitted from activities use the source name as-is — there is no automatic namespace prefix. Activities must include the full source name to avoid collisions across agents:

```
emit(source: "chief-of-staff.briefing.ready", payload: {...})
```

**Convention:** Prefix event sources with the agent slug to prevent cross-agent collisions. Other agents subscribe to the full source name.

### Emitting from Activities

```
emit(source: "chief-of-staff.email.customer-service", payload: {"from": "j@example.com", "subject": "..."})
```

Each `emit` call is a discrete event. If an activity emits 5 events, each fires independently against all active subscriptions. This enables **fan-out pipelines**.

### System Events

| Event | When |
|-------|------|
| `workflow.{id}.completed` | A workflow run finishes successfully |
| `workflow.{id}.failed` | A workflow run fails |

System events enable workflow chaining: A completes → B triggers.

### Progress Events

In addition to completion events, the engine emits progress events for live monitoring:

| Event | When | Payload |
|-------|------|---------|
| `ActivityStarted` | Each activity begins executing | Activity ID, workflow run ID |
| `TaskUpdated` | During activity execution | Progress details, current activity state |

These events are broadcast over WebSocket, enabling the UI to show real-time workflow progress.

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

## Posting to a Loop Channel

A workflow activity can deliver its output to a **loop channel** — a shared NeboAI channel that other agents (and people) can read — instead of (or in addition to) the chat. The agent does this with the `loop` tool, and it does **not** need a human to pre-create the channel.

Two calls cover the whole pattern:

| Call | What it does |
|------|--------------|
| `loop(resource: "channel", action: "ensure", name: "daily-briefing", description: "Morning briefing")` | Find-or-create. Returns a `channel_id`. Idempotent — creates `#daily-briefing` the first time, and returns the **same** `channel_id` on every subsequent run. No duplicate channels. |
| `loop(resource: "channel", action: "send", channel_id: "<from ensure>", text: "Good morning ☀️ …")` | Posts a message to that channel. `send` requires a `channel_id` — that's what `ensure` hands back. |

Chain them: `ensure` first to get the `channel_id`, then `send` to post. Because `ensure` is idempotent, you run both every time the workflow fires — there's no separate "set up the channel once" step and nothing for the publisher or end user to configure. Given an intent like *"ensure the `#daily-briefing` channel exists, then post the briefing to it,"* the activity chains the two calls itself.

> The `loop` tool is always available to activities — no `skills` or `mcps` entry needed. See [Channel Plugins](channel-plugins.md) for posting to external surfaces like Slack or Discord, which is a different pathway.

### Example: Chief of Staff Daily Briefing

A scheduled workflow that ensures `#daily-briefing` exists and posts a morning briefing to it every weekday at 7am:

```json
{
  "workflows": {
    "daily-briefing": {
      "trigger": { "type": "schedule", "cron": "0 7 * * 1-5" },
      "description": "Post a morning briefing to the #daily-briefing loop channel",
      "activities": [
        {
          "id": "gather",
          "intent": "Gather today's priorities from calendar and email",
          "skills": ["@chief-of-staff/skills/briefing-writer"],
          "steps": [
            "Check calendar for today's meetings",
            "Scan inbox for urgent items",
            "Compose a concise briefing as bullet points"
          ],
          "token_budget": { "max": 4096 }
        },
        {
          "id": "post",
          "intent": "Publish the briefing to the #daily-briefing loop channel",
          "steps": [
            "Ensure the channel exists: loop(resource: \"channel\", action: \"ensure\", name: \"daily-briefing\", description: \"Daily morning briefing\") — this returns a channel_id",
            "Post the briefing from the previous activity: loop(resource: \"channel\", action: \"send\", channel_id: <id from ensure>, text: <the briefing>)"
          ],
          "token_budget": { "max": 1024 }
        }
      ],
      "budget": { "total_per_run": 6000 },
      "emit": "chief-of-staff.briefing.posted"
    }
  }
}
```

Every weekday at 7am the `gather` activity composes the briefing, then `post` ensures `#daily-briefing` (creating it on day one, reusing it thereafter) and sends the briefing into it. The `emit` fires `chief-of-staff.briefing.posted` so downstream agents can react.

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
- `activities` must be present (defines what runs) — bindings are always inline
- Trigger `type` must be one of: `schedule`, `heartbeat`, `event`, `watch`, `folder`, `manual`
- **Lenient parsing:** Individual workflow bindings that fail to parse (e.g., invalid trigger format) are skipped with a warning — they do not prevent the agent from loading. The agent still appears in the UI with its remaining valid workflows.
- Schedule triggers must have a valid 5-field `cron` expression
- Heartbeat triggers must have a valid `interval` (e.g., `"30m"`, `"1h"`)
- Event triggers should have at least one entry in `sources`
- Watch triggers must have a `plugin` and either `event` or `command`
- Activity IDs must be unique within each binding
- If `budget.total_per_run > 0`, the sum of all activity `token_budget.max` must not exceed it
- All `{{key}}` placeholders in commands must match an input `key` exactly
- An Agent with no workflows is valid (chat-only + persona + skills)
