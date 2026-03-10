# Roles (`@org/roles/name`)

A Role is a job description with a schedule. It bundles workflows and skills into a complete job profile — and it defines *when* each workflow runs. The Role is the only artifact type that owns event bindings.

A Role is three files: `manifest.json` (identity), `role.json` (operational wiring), and `ROLE.md` (persona).

For packaging format and manifest.json, see [Packaging](packaging.md).

---

## role.json — The Job Definition

The `role.json` carries the operational structure: which workflows to run, when to run them, and what events to listen for. This is the file that makes a Role more than a folder of workflows — it's what makes it an employee who already knows the job.

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
  "skills": [
    "@nebo/skills/briefing-writer@^1.0.0"
  ],
  "pricing": {
    "model": "monthly_fixed",
    "cost": 47.0
  },
  "defaults": {
    "timezone": "user_local",
    "configurable": ["workflows.morning-briefing.trigger.cron", "workflows.evening-wrap.trigger.cron", "workflows.day-monitor.trigger.interval"]
  }
}
```

---

## role.json Fields

### Top-Level

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `workflows` | map | no | `{}` | Workflow bindings with triggers (keyed by binding name) |
| `skills` | string[] | no | `[]` | Additional skill qualified names (beyond what workflows declare) |
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
| `configurable` | string[] | JSON paths within `role.json` that the user can override after installation. |

### Workflow Binding

Each entry in the `workflows` map binds a workflow to a trigger:

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `ref` | string | yes | — | Workflow qualified name (`@org/workflows/name@version`) |
| `trigger` | object | yes | — | When this workflow runs |
| `description` | string | no | `""` | Human-readable description of this binding |
| `inputs` | map | no | `{}` | Default inputs passed to the workflow on trigger |

### Trigger Types

| Type | Fields | Description |
|------|--------|-------------|
| `schedule` | `cron` (string) | Fires on a cron schedule. Standard 5-field cron expression. |
| `heartbeat` | `interval` (string), `window` (string, optional) | Fires at a recurring interval. Window limits active hours (e.g., `"08:00-18:00"`). |
| `event` | `sources` (string[]) | Fires when a matching event occurs. See [Event System](#event-system) below. |
| `manual` | — | Only fires by explicit user request or API call. |

> **Key principle:** The workflow doesn't decide when it runs. The Role does. The same `@acme/workflows/lead-qualification` workflow could run at 7am in one Role and 9am in another. The procedure doesn't change just because you want your briefing at a different time.

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

When an event is emitted, Nebo checks all active event trigger subscriptions across all installed Roles. Every matching subscription spawns a new workflow run with the event's `payload` as inputs.

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

## ROLE.md — The Persona

The `ROLE.md` is the agent's job description in prose. It defines who the agent *is* when operating in this Role — personality, communication style, priorities, judgment calls.

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

When a user installs a Role:

1. Parse `role.json` for all workflow references
2. For each workflow: resolve version, install its declared dependencies (skills, sub-workflows)
3. Install any additional skills listed directly in `role.json`
4. Register all trigger bindings from `role.json`
5. Load the ROLE.md persona into the agent's context

The user installs a job. Everything else cascades.

---

## Validation Rules

- Each workflow binding must have a valid `ref` (qualified name: `@org/workflows/name@version`) and a `trigger`
- Trigger type must be one of: `schedule`, `heartbeat`, `event`, `manual`
- Schedule triggers must have a valid `cron` expression
- Heartbeat triggers must have a valid `interval` (e.g., `"30m"`, `"1h"`)
- Event triggers must have at least one entry in `sources`
- Skill refs must be qualified names (`@org/skills/name`)
- A Role with no workflows is valid — it provides only a persona and skill declarations
