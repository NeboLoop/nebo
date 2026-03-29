# Workflow Developer Guide

This guide covers everything you need to design, build, test, and deploy workflows for Nebo. A workflow is a JSON definition that chains AI-powered activities together with tool access, budget constraints, and error handling. Workflows are pure procedures — they define *how* something gets done, not *when*. Triggers and schedules belong to the Agent that binds the workflow. Workflows run as background subagents with their own tool access and token budgets, independent from the main agent conversation.

## When to Use a Workflow

Workflows are the right choice when you need:

- **Repeatable multi-step automation** — a process that runs the same way every time
- **Scheduled tasks** — something that runs on a cron schedule without human involvement
- **Event-driven pipelines** — reacting to external events (new lead, email received, etc.)
- **Cost-controlled execution** — hard token budgets per activity and per run
- **Deterministic skill access** — each activity declares exactly which skills it can use

If you just need a one-shot task, use the agent directly. Workflows are for processes you want to define once and run many times.

---

## Anatomy of a Workflow

A workflow is a single JSON document (`workflow.json`) with this top-level structure:

```json
{
  "version": "1",
  "id": "lead-qualification",
  "name": "Lead Qualification",
  "inputs": {...},
  "activities": [...],
  "dependencies": {...},
  "budget": {...}
}
```

### Top-Level Fields

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `version` | string | yes | — | Schema version (currently `"1"`) |
| `id` | string | yes | — | Local workflow identifier used by the engine (lowercase, hyphens) |
| `name` | string | yes | — | Human-readable display name |
| `inputs` | map | no | `{}` | Input parameter definitions |
| `activities` | Activity[] | yes | — | Ordered list of execution steps |
| `dependencies` | Dependencies | no | `{}` | Skills and workflows required |
| `budget` | Budget | no | `{}` | Token budget constraints |

> **`id` vs manifest `name`:** The `id` field in workflow.json is the local engine identifier — it's how the REST API addresses the workflow, how run records are keyed, and how the `work` tool references it. The `name` field in `manifest.json` is the marketplace identity (`@org/workflows/name`). They serve different purposes. The `id` is typically the last segment of the qualified name (e.g., `lead-qualification` from `@acme/workflows/lead-qualification`).

> **Note:** There is no `triggers` field in workflow.json. Workflows are pure procedures. Triggers and schedules are defined in the Agent's `agent.json`, which binds workflows to events. This means the same workflow can run on different schedules in different Agents, and a user can change when something runs without touching the procedure itself. A standalone workflow without an Agent can still be invoked manually or via the `work` tool.

---

## Inputs

Inputs define the parameters a workflow accepts. They are passed when the workflow is run and made available to every activity in the `## Inputs` section of the system prompt.

```json
"inputs": {
  "lead_email": {
    "type": "string",
    "required": true
  },
  "priority": {
    "type": "string",
    "required": false,
    "default": "normal"
  },
  "max_results": {
    "type": "integer",
    "required": false,
    "default": 10
  }
}
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `type` | string | yes | — | `"string"`, `"integer"`, `"number"`, `"boolean"`, `"array"`, `"object"` |
| `required` | bool | no | `false` | Whether the input must be provided |
| `default` | any | no | `null` | Fallback value if not provided |

Inputs are injected into each activity's system prompt as a formatted list:

```
## Inputs
- lead_email: "user@example.com"
- priority: "high"
```

---

## Activities

Activities are the core execution units. They run **sequentially** — each activity completes before the next one starts. Each activity is an independent AI call with its own system prompt, skill access, model selection, and token budget.

```json
{
  "id": "research",
  "intent": "Research the lead's company and recent news",
  "skills": [
    "@acme/skills/sales-qualification@^1.0.0",
    "@acme/skills/crm-lookup@^1.0.0"
  ],
  "bindings": [
    { "interface": "crm-lookup" }
  ],
  "model": "claude-sonnet",
  "steps": [
    "Look up the lead's company using the CRM skill",
    "Search for recent news and funding rounds",
    "Summarize key findings in 3-5 bullet points"
  ],
  "token_budget": {
    "max": 8192
  },
  "on_error": {
    "retry": 2,
    "fallback": "skip"
  }
}
```

### Activity Fields

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `id` | string | yes | — | Unique within the workflow |
| `intent` | string | yes | — | What this activity should accomplish |
| `skills` | string[] | no | `[]` | Skill qualified names to activate (provides knowledge + actions) |
| `bindings` | Binding[] | no | `[]` | Interface bindings for portable skill references |
| `model` | string | no | `""` | AI model (e.g., `"claude-sonnet"`, `"claude-haiku"`) |
| `steps` | string[] | no | `[]` | Numbered instructions for the AI |
| `token_budget.max` | u32 | no | `4096` | Maximum tokens for this activity |
| `on_error.retry` | u32 | no | `1` | Number of attempts (1 = no retries) |
| `on_error.fallback` | string | no | `"notify_owner"` | What to do after all retries fail |

### How Activities Execute

Each activity follows this execution loop:

1. **Build system prompt** from intent, steps, inputs, and prior activity results
2. **Filter skills** — only skills declared in the activity's `skills` array (and resolved bindings) are available
3. **Send to AI** — calls the provider with `temperature: 0.0` (deterministic)
4. **Execution loop** — if the AI requests actions (skill scripts, platform capabilities), execute them and continue (max 20 iterations)
5. **Budget check** — hard stop if token budget is exceeded
6. **Return** — the response text becomes the activity's result

The activity's result is passed to subsequent activities as context:

```
## Prior Results
[Activity 'research' result]: The company was founded in 2019...
[Activity 'qualify' result]: Lead score: 8/10, qualified for enterprise tier...
```

### Lean Execution

Activities run in a **lean execution path** — there is no steering, no memory injection, no personality context, and no conversation history from the main agent. The only context is:

- Skill bodies (if the activity declares skills)
- The activity's intent and steps
- The workflow inputs
- Results from prior activities

This is by design: workflows are deterministic, repeatable processes.

> **Note on Agents:** Installing an Agent loads the AGENT.md persona into the bot's *conversational* context — it shapes how the bot talks to the user. But the Agent's own workflows do **not** inherit that persona. Workflow activities get only the lean context listed above. The persona is for the bot's personality in chat; workflows are impersonal execution engines.

### Activity ID Rules

- Must not be empty
- Must be unique within the workflow
- Used as keys in the run record, error reporting, and prior results context

---

## Skill References

Activities declare which skills they can use. Skills provide both knowledge (injected into the system prompt) and actions (scripts, API calls via platform capabilities). If an activity declares no skills, it runs with only the intent, steps, and inputs — no domain knowledge or actions.

### Qualified Name (Direct)

Pin the activity to a specific skill by its qualified name:

```json
"skills": ["@acme/skills/crm-lookup@^1.0.0"]
```

The version specifier follows semver range semantics.

### Interface Binding

Declare an abstract interface. The user configures which concrete skill implements it via the bindings API:

```json
"bindings": [{ "interface": "crm-lookup" }]
```

This makes the workflow portable — different users can bind "crm-lookup" to Salesforce, HubSpot, or any other CRM skill.

### Mixing Formats

You can use both direct references and interface bindings in the same activity:

```json
"skills": ["@acme/skills/sales-qualification@^1.0.0"],
"bindings": [{ "interface": "crm-lookup" }]
```

### Skill Filtering

At runtime, the engine activates only the skills declared in the activity's `skills` array plus any resolved bindings.

---

## Interface Bindings

Interface bindings decouple a workflow from specific skills. The workflow author declares abstract interfaces; the user maps them to concrete skills.

### Setting Bindings (REST API)

```
PUT /workflows/{id}/bindings
```

```json
{
  "bindings": [
    { "interfaceName": "crm-lookup", "skill": "@acme/skills/crm-lookup@^1.0.0" },
    { "interfaceName": "email-send", "skill": "@acme/skills/email-service@^1.0.0" }
  ]
}
```

### Reading Bindings

```
GET /workflows/{id}/bindings
```

```json
{
  "bindings": [
    { "id": 1, "workflowId": "lead-qualification", "interfaceName": "crm-lookup", "skill": "@acme/skills/crm-lookup@1.0.0" },
    { "id": 2, "workflowId": "lead-qualification", "interfaceName": "email-send", "skill": "@acme/skills/email-service@1.0.0" }
  ],
  "total": 2
}
```

Bindings are stored per-workflow. Updating bindings replaces all existing bindings for that workflow.

### Unresolved Bindings

If an activity references `{ "interface": "crm-lookup" }` and no binding exists for that interface, the skill is silently excluded from the activity. The activity runs without it. If the activity depends on the skill to accomplish its intent, it will fail at the AI level and the `on_error` policy applies.

---

## Error Handling

Each activity has its own error handling policy: retry count and fallback strategy.

### Retry

```json
"on_error": {
  "retry": 3
}
```

The `retry` field is the total number of attempts. `1` means no retries (try once). `3` means try up to 3 times. If an attempt succeeds, the activity proceeds normally. If all attempts fail, the fallback strategy kicks in.

### Fallback Strategies

| Strategy | Behavior |
|----------|----------|
| `notify_owner` | Record the failure, stop the workflow, return error. (Default) |
| `skip` | Log a warning, skip this activity, continue to the next one |
| `abort` | Stop the entire workflow immediately, record failure |

Both `notify_owner` and `abort` currently stop the workflow and record the failure. In a future release, `notify_owner` will additionally send a notification to the workflow owner. Use `notify_owner` for failures a human should review; use `abort` for hard stops where no human action is needed.

### Error Types

| Error | Cause |
|-------|-------|
| `BudgetExceeded` | Activity or workflow token budget exceeded |
| `MaxIterations` | Activity execution loop exceeded 20 iterations |
| `ActivityFailed` | AI provider returned an error |
| `Provider` | Could not connect to AI provider |

---

## Budget Constraints

Workflows enforce token budgets at two levels.

### Activity Budget

Each activity has a `token_budget.max` (default: 4096 tokens). This is a hard limit — if the activity exceeds it, execution stops with a `BudgetExceeded` error.

```json
"token_budget": { "max": 8192 }
```

Token usage is tracked across all iterations of the execution loop within an activity.

### Workflow Budget

The top-level `budget.total_per_run` is the total token limit across all activities. After each activity completes, the engine checks whether cumulative usage exceeds this limit.

```json
"budget": {
  "total_per_run": 32768,
  "cost_estimate": "$0.10"
}
```

The `cost_estimate` field is informational — it is not enforced. It helps users understand the expected cost.

### Validation

If `total_per_run > 0`, the sum of all activity `token_budget.max` values must not exceed it. This is checked at parse time.

---

## Dependencies

The `dependencies` section declares what the workflow needs to run. These are checked and auto-installed when the workflow is installed from the marketplace.

> **`dependencies` vs activity fields:** The `dependencies.skills` list declares what must be *installed* for the workflow to run. The `skills` field on individual activities declares what is *available to each activity* at runtime. A skill should appear in both: in `dependencies` to ensure it's installed, and in the activity's `skills` array to inject it into that activity's context. If a skill is in `dependencies` but not in any activity, it's installed but never used. If it's in an activity but not in `dependencies`, it may be missing at runtime.

```json
"dependencies": {
  "skills": [
    "@acme/skills/sales-qualification@^1.0.0",
    "@acme/skills/crm-lookup@^1.0.0",
    "@acme/skills/email-service@^1.0.0"
  ],
  "workflows": []
}
```

| Field | Type | Description |
|-------|------|-------------|
| `skills` | string[] | Skill qualified names that must be installed |
| `workflows` | string[] | Nested workflow qualified names |

---

## Complete Example

Here is a full workflow that qualifies inbound leads:

```json
{
  "version": "1",
  "id": "lead-qualification",
  "name": "Lead Qualification",
  "inputs": {
    "lead_email": {
      "type": "string",
      "required": true
    },
    "priority": {
      "type": "string",
      "required": false,
      "default": "normal"
    }
  },
  "activities": [
    {
      "id": "research",
      "intent": "Research the lead's company, role, and recent activity",
      "bindings": [
        { "interface": "crm-lookup" }
      ],
      "model": "claude-sonnet",
      "steps": [
        "Look up the lead by email in the CRM",
        "Search for their company's recent news and funding",
        "Check LinkedIn for the lead's current role and tenure",
        "Summarize findings in a structured format"
      ],
      "token_budget": { "max": 8192 },
      "on_error": {
        "retry": 2,
        "fallback": "skip"
      }
    },
    {
      "id": "qualify",
      "intent": "Score the lead and decide on next steps",
      "skills": ["@acme/skills/sales-qualification@^1.0.0"],
      "bindings": [
        { "interface": "crm-lookup" }
      ],
      "model": "claude-sonnet",
      "steps": [
        "Based on the research, score the lead 1-10 on fit and intent",
        "If score >= 7, mark as MQL and recommend immediate outreach",
        "If score 4-6, mark as nurture and suggest a drip campaign",
        "If score <= 3, mark as disqualified with reason",
        "Update the CRM with the qualification status and notes"
      ],
      "token_budget": { "max": 4096 },
      "on_error": {
        "retry": 1,
        "fallback": "notify_owner"
      }
    },
    {
      "id": "notify",
      "intent": "Send a summary to the sales team",
      "bindings": [
        { "interface": "email-send" }
      ],
      "model": "claude-haiku",
      "steps": [
        "Draft a brief email summarizing the lead qualification",
        "Include the score, key findings, and recommended action",
        "Send to the sales team distribution list"
      ],
      "token_budget": { "max": 2048 },
      "on_error": {
        "retry": 1,
        "fallback": "skip"
      }
    }
  ],
  "dependencies": {
    "skills": [
      "@acme/skills/sales-qualification@^1.0.0",
      "@acme/skills/crm-lookup@^1.0.0",
      "@acme/skills/email-service@^1.0.0"
    ],
    "workflows": []
  },
  "budget": {
    "total_per_run": 16384,
    "cost_estimate": "$0.05"
  }
}
```

### What Happens When This Runs

1. **research** — Looks up the lead in the CRM, searches for company news. If it fails after 2 attempts, it skips (the next activity proceeds without research context).

2. **qualify** — Reads the research results from `## Prior Results`, scores the lead, updates the CRM. If it fails, the workflow stops and the owner is notified.

3. **notify** — Reads both prior results, drafts and sends a summary email. If it fails, it skips (the qualification was already done).

Budget cap: 16,384 tokens total per run (activity budgets sum to 14,336 — the headroom covers retries). Each activity has its own sub-budget. Temperature is 0.0 (deterministic) for all activities.

---

## Running Workflows

### Via the Agent (work tool)

The agent uses the `work` tool to manage and run workflows. `work` follows the STRAP pattern (Single Tool Resource Action Pattern) — instead of one tool per operation, it's one tool per domain with `resource`, `action`, and `inputs` parameters.

#### Action Reference

| Action | Resource | Description |
|--------|----------|-------------|
| `list` | — | List all installed workflows |
| `run` | workflow id | Execute a workflow (returns run_id immediately) |
| `status` | workflow id | Check latest run status |
| `runs` | workflow id | List recent runs |
| `toggle` | workflow id | Enable or disable a workflow |
| `cancel` | run id | Cancel a running workflow (stops between activities) |
| `install` | — | Install from marketplace (pass `name` with qualified name) |
| `uninstall` | workflow id | Remove a workflow |

Examples:

```
# List all installed workflows
work(action: "list")

# Run a workflow by name
work(resource: "lead-qualification", action: "run", inputs: {"lead_email": "j@example.com"})

# Check latest run status
work(resource: "lead-qualification", action: "status")

# List recent runs
work(resource: "lead-qualification", action: "runs")

# Enable or disable
work(resource: "lead-qualification", action: "toggle")
```

The `run` action returns a `run_id` immediately — the workflow executes in the background.

### Via REST API

```
POST /workflows/{id}/run
Content-Type: application/json

{
  "inputs": {
    "lead_email": "j@example.com",
    "priority": "high"
  }
}
```

Response:

```json
{
  "run": {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "workflowId": "lead-qualification",
    "triggerType": "manual",
    "status": "running",
    "startedAt": 1709740800
  }
}
```

### Via Schedule (Agent-Bound)

Workflows do not carry their own schedule triggers. To run a workflow on a schedule, bind it to an Agent with a `schedule` trigger in `agent.json`. The scheduler polls every 60 seconds and fires workflows when their cron expression is due. Scheduled runs use `trigger_type: "cron"`. If the Agent's workflow binding defines default `inputs`, those are passed to the workflow; otherwise the workflow receives no inputs.

---

## Installation

### From the Marketplace

Install a workflow using its qualified name or install code:

```
work(action: "install", name: "@acme/workflows/lead-qualification@^1.0.0")
```

Or paste an install code in chat — Nebo detects `WORK-XXXX-XXXX` codes automatically, resolves them to the qualified name, and installs the workflow.

The marketplace resolves the reference, downloads the workflow definition, stores it in the local database, and makes it available for manual invocation or Agent binding.

### Via REST API

Create a workflow directly:

```
POST /workflows
Content-Type: application/json

{
  "definition": "{...workflow.json as string...}",
  "workflowMd": "...",
  "manifest": "{...manifest.json as string...}"
}
```

The `manifest` field carries the package identity (qualified name, version). The `definition` field carries the workflow procedure (workflow.json). The `workflowMd` field carries the agent documentation. All three are validated on creation — if any fails parsing or validation, the request is rejected.

### Uninstalling

```
work(action: "uninstall", id: "lead-qualification")
```

Or via REST API:

```
DELETE /workflows/{id}
```

Uninstalling removes the workflow, all its bindings, and deregisters it from any Agents that reference it.

---

## Monitoring Runs

### Run Status

Each workflow run has a status:

| Status | Meaning |
|--------|---------|
| `running` | Currently executing |
| `completed` | All activities finished successfully |
| `failed` | An activity failed and the fallback was `abort` or `notify_owner` |
| `cancelled` | Cancelled by user request — stopped between activities |

### Run Record

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "workflowId": "lead-qualification",
  "triggerType": "manual",
  "triggerDetail": null,
  "status": "completed",
  "inputs": "{\"lead_email\":\"j@example.com\"}",
  "currentActivity": null,
  "totalTokensUsed": 12847,
  "error": null,
  "errorActivity": null,
  "sessionKey": "workflow-lead-qualification-550e8400...",
  "startedAt": 1709740800,
  "completedAt": 1709740823
}
```

### Activity Results

Each activity's execution is recorded separately:

```json
{
  "id": 1,
  "runId": "550e8400...",
  "activityId": "research",
  "status": "completed",
  "tokensUsed": 6241,
  "attempts": 1,
  "error": null,
  "startedAt": 1709740800,
  "completedAt": 1709740812
}
```

### REST API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/workflows` | List all workflows |
| `POST` | `/workflows` | Create a workflow |
| `GET` | `/workflows/{id}` | Get workflow details |
| `PUT` | `/workflows/{id}` | Update workflow definition |
| `DELETE` | `/workflows/{id}` | Delete workflow |
| `POST` | `/workflows/{id}/toggle` | Enable/disable |
| `POST` | `/workflows/{id}/run` | Run workflow |
| `GET` | `/workflows/{id}/runs` | List runs |
| `GET` | `/workflows/{id}/runs/{runId}` | Get run with activity results |
| `POST` | `/workflows/{id}/runs/{runId}/cancel` | Cancel a running workflow |
| `GET` | `/workflows/{id}/bindings` | List interface bindings |
| `PUT` | `/workflows/{id}/bindings` | Set interface bindings |

### WebSocket Events

Workflow lifecycle events are broadcast over WebSocket:

| Event | When |
|-------|------|
| `workflow_installed` | Workflow created or installed |
| `workflow_uninstalled` | Workflow deleted |
| `workflow_run_completed` | Run finished successfully |
| `workflow_run_failed` | Run failed |
| `workflow_run_cancelled` | Run cancelled by user request |

---

## Validation Rules

The parser enforces these rules at creation time:

1. `id` must not be empty
2. `name` must not be empty
3. At least one activity is required
4. Every activity `id` must not be empty
5. Activity IDs must be unique within the workflow
6. If `budget.total_per_run > 0`, the sum of all activity `token_budget.max` values must not exceed it

---

## WORKFLOW.md

Every workflow package includes a `WORKFLOW.md` file alongside `manifest.json` and `workflow.json`. This is the workflow's documentation for the agent — it tells the agent what the workflow does, when it's appropriate to use, and how to interpret its results. It is plain markdown with no frontmatter (identity comes from the manifest).

This is analogous to `SKILL.md` in a skill package and `AGENT.md` in an agent package.

---

## Design Guidelines

### Keep Activities Focused

Each activity should do one thing. A "research, qualify, and notify" mega-activity will be harder to debug, retry, and budget than three separate activities.

### Use Interface Bindings for Portability

If your workflow will be shared on the marketplace, use interface bindings instead of hardcoded skill names. This lets each user bind their own skills (e.g., different CRM providers).

### Set Meaningful Token Budgets

The default is 4,096 tokens per activity. Research-heavy activities may need 8,192 or 16,384. Simple formatting or notification activities can use 1,024 or 2,048. Setting tight budgets prevents runaway costs.

### Choose the Right Fallback

- Use `skip` for non-critical activities (notifications, logging)
- Use `notify_owner` for important activities where a human should review the failure
- Use `abort` for critical activities where continuing would produce incorrect results

### Use Steps for Determinism

The `steps` array is injected as numbered instructions in the system prompt. More specific steps produce more consistent results across runs. Vague intents like "handle the lead" are less reliable than explicit step-by-step instructions.

### Model Selection

Pick the cheapest model that can handle the activity:

- **claude-haiku** — formatting, summarization, simple lookups
- **claude-sonnet** — analysis, research, multi-step reasoning
- **claude-opus** — complex judgment calls, creative tasks

If `model` is empty, the system's default model is used.

### Budget Math

Plan your budget from the bottom up:

```
Activity 1: research    → 8,192 tokens
Activity 2: qualify     → 4,096 tokens
Activity 3: notify      → 2,048 tokens
                          ──────
Sum:                      14,336 tokens
total_per_run:            16,384 tokens (headroom for retries)
```

The `total_per_run` should be at least the sum of all activity budgets, with some headroom if you use retries.

---

## Emitting Events

Workflow activities can emit events using the built-in `emit` tool. This is how workflows feed data into the event system — enabling fan-out pipelines, workflow chaining, and event-driven architectures.

### The `emit` Tool

The `emit` tool is a built-in tool available to any workflow activity. It does not need to be declared in `dependencies` or the activity's `skills` array — it is always available.

```
emit(source: "email.customer-service", payload: {"from": "j@example.com", "subject": "Order issue"})
```

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `source` | string | yes | Event type string — matched against trigger `sources` in active Agents |
| `payload` | map | no | Arbitrary data — becomes the triggered workflow's `inputs` |

Each `emit` call produces a discrete event. If an activity emits 5 events, each one is matched independently against all active trigger subscriptions across all installed Agents.

### Fan-Out Pattern

The primary use case for `emit` is fan-out: a triage workflow processes a batch, classifies items, and emits one event per item. Each event triggers the appropriate handler workflow with that item's data as inputs.

For example, an email triage workflow reads the inbox and classifies each email:

```
emit(source: "email.customer-service", payload: {"from": "j@example.com", "subject": "Order issue", "body": "..."})
emit(source: "email.sales-inquiry", payload: {"from": "k@example.com", "subject": "Pricing", "body": "..."})
```

Each emit triggers whatever Agent workflow is subscribed to that source. See [Agents — Event System](agents.md#event-system) for the full event architecture.

---

## Execution Internals

For workflow developers who want to understand exactly what happens at runtime.

### Execution Flow

1. Fetch workflow from DB, verify it is enabled
2. Parse the definition via `workflow::parser::parse_workflow()`
3. Create a `WorkflowRun` record with status `"running"`
4. Spawn a background task (`tokio::spawn`) — the caller gets the `run_id` immediately
5. In the background task:
   a. Acquire an AI provider (first available from the shared provider list)
   b. Resolve all skill references and interface bindings
   c. Iterate through activities sequentially
   d. **Before each activity**: check the cancellation flag. If set, stop execution, update run to `"cancelled"`, broadcast `workflow_run_cancelled` WebSocket event, and return
   e. For each activity: filter skills, build prompt, execute with retry
   f. Accumulate token usage, check budget after each activity
   g. Record activity results in DB
6. On completion: update run to `"completed"`, broadcast WebSocket event
7. On failure: update run to `"failed"` with error message, broadcast event
8. On cancellation: update run to `"cancelled"`, broadcast event

### System Prompt Structure

Each activity gets a system prompt built from five sections:

```
## Skills
[Contents of @acme/skills/sales-qualification SKILL.md body]

## Task
Research the lead's company and recent news

## Steps
1. Look up the lead's company using the CRM skill
2. Search for recent news and funding rounds
3. Summarize key findings in 3-5 bullet points

## Inputs
- lead_email: "j@example.com"
- priority: "high"

## Prior Results
[Activity 'research' result]: The company was founded in 2019...
```

If the activity declares `skills`, the body of each SKILL.md is injected into the `## Skills` section at the top of the prompt. This is how domain knowledge enters the activity's context. If no skills are declared, the section is omitted.

There is no personality, memory, steering, or conversation history.

### Execution Loop

Within each activity, the engine runs an execution loop:

1. Send messages to the AI provider
2. If the AI requests actions (via skill scripts or platform capabilities), execute them
3. Append results to the message history
4. Repeat (up to 20 iterations)
5. If the AI returns text with no further actions, the activity is complete

Temperature is always `0.0` for deterministic output.
