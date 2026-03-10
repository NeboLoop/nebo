# Nebo Platform Taxonomy: Skills, Workflows, and Roles

**Document Type:** SME Architecture Reference
**Status:** Canonical
**Discussion Reference:** 4743279a-d5df-4629-a7bc-622ef7a9dc85

---

## Overview

Nebo's platform is built around five primitive objects, each with a unique install code and a defined role in the hierarchy:

| Code Prefix | Primitive | What It Is |
|-------------|-----------|------------|
| `SKIL-XXXX-XXXX-XXXX` | Skill | Knowledge the agent loads into context |
| `WORK-XXXX-XXXX-XXXX` | Workflow | A procedure composed of activities |
| `ROLE-XXXX-XXXX-XXXX` | Role | A job with a schedule — binds workflows to triggers |
| `NEBO-XXXX-XXXX-XXXX` | Agent (Nebo) | The agent instance that runs everything |
| `LOOP-XXXX-XXXX-XXXX` | Community | The NeboLoop community where primitives are shared |

A **Role** is a job description with a schedule. It bundles Workflows and Skills into a complete job profile and — critically — defines *when* each workflow runs. Roles own the event bindings. Workflows are pure procedures with no opinion about timing. This separation means the same workflow can run on different schedules in different Roles, and users can change when something runs without touching the procedure itself.

---

## 1. Skills

### What a Skill is

A Skill is pure knowledge. It is training, expertise, and domain context that an agent loads into its working memory when it needs to reason well about a specific type of task. A Skill does not execute anything. It does not call APIs. It makes the agent smarter.

A Skill is **a single file**: `SKILL.md`.

### When Skills are used

Skills are injected into the agent's context at the point where they are needed — when a Workflow activity specifies them for a particular step. The agent reads the Skill as part of its context window and uses that knowledge to inform its reasoning and output quality.

### File structure

```
SKIL-XXXX-XXXX-XXXX/
└── SKILL.md
```

The `SKILL.md` is plain markdown. It can include:

- Domain knowledge (e.g., "here is how to score a sales lead")
- Tone and style guides (e.g., "here is how we communicate with clients")
- Rules and heuristics (e.g., "here are the criteria for qualifying a prospect")
- Procedural knowledge (e.g., "here is how to handle an edge case in tax law")

### Examples

- `SKIL-sales-qualification` — How to evaluate whether a lead is worth pursuing
- `SKIL-onboarding-tone` — How to write warm, professional onboarding communications
- `SKIL-tax-law-schedule-c` — Current tax code guidance for Schedule C filers
- `SKIL-industry-knowledge-realestate` — Real estate market context and terminology

### Installation

```go
InstallSkill(ctx, "SKIL-XXXX-XXXX-XXXX")
```

Skills are free, lightweight, and always auto-installed when a Workflow depends on them. There is no reason to present the user with a choice about Skill installation.

---

## 2. Workflows

### What a Workflow is

A Workflow is a pure procedure. It defines the repeatable steps required to accomplish a specific task — decomposed into **activities**, each with its own intent, skills, model selection, and token budget. A Workflow has no opinion about *when* it runs. It defines *how* something gets done. Triggers and schedules belong to the Role that binds the workflow.

A Workflow is three files: `manifest.json` + `workflow.json` + `WORKFLOW.md`.

### File structure

```
WORK-XXXX-XXXX-XXXX/
├── manifest.json     — Universal identity envelope
├── workflow.json     — The procedure definition (activities, steps, budgets)
└── WORKFLOW.md       — What this workflow does, how the agent should approach it
```

### The three layers of a Workflow

A well-designed workflow gives the agent everything it needs to do a job reliably and intelligently:

1. **Specialized knowledge** — Skills loaded per activity give the agent domain expertise
2. **Repeatable procedure** — Steps within each activity define what to do, in order, every time
3. **Autonomy** — The agent's intent-driven reasoning means it handles edge cases intelligently without rigid hardwiring

### workflow.json schema

> **Note:** There is no `triggers` field. Workflows are pure procedures. The Role's `role.json` binds workflows to triggers. This means the same workflow can run on different schedules in different Roles, and a user can change when something runs without touching the procedure itself.

```json
{
  "version": "1.0",
  "id": "lead-qualification",
  "name": "Lead Qualification",

  "inputs": {
    "client_name": { "type": "string", "required": true },
    "client_email": { "type": "string", "required": true },
    "inquiry_type": { "type": "string", "required": false }
  },

  "activities": [
    {
      "id": "lookup",
      "intent": "Find existing contact record and history",
      "skills": [],
      "model": "haiku",
      "steps": [
        "Search CRM by email address",
        "Pull contact history and previous interactions",
        "Check for any existing deals or opportunities"
      ],
      "token_budget": { "max": 1500 },
      "on_error": { "retry": 2, "fallback": "notify_owner" }
    },
    {
      "id": "assess",
      "intent": "Evaluate lead quality based on all available signals",
      "skills": ["SKIL-sales-qualification", "SKIL-industry-knowledge"],
      "model": "sonnet",
      "steps": [
        "Review inquiry type against ideal client profile",
        "Check serviceable geographic area",
        "Verify not previously declined",
        "Assess budget signals from inquiry details",
        "Score lead on 1-10 scale with reasoning"
      ],
      "token_budget": { "max": 4000 },
      "on_error": { "retry": 1, "fallback": "notify_owner" }
    },
    {
      "id": "route",
      "intent": "Take appropriate action based on qualification score",
      "skills": ["SKIL-onboarding-tone"],
      "model": "haiku",
      "steps": [
        "If score >= 7, tag as qualified in CRM",
        "If score >= 7, send confirmation to intake team",
        "If score < 7, draft polite referral email with alternatives",
        "Log qualification decision and reasoning in CRM"
      ],
      "token_budget": { "max": 2000 },
      "on_error": { "retry": 2, "fallback": "notify_owner" }
    }
  ],

  "dependencies": {
    "skills": ["SKIL-sales-qualification", "SKIL-industry-knowledge", "SKIL-onboarding-tone"],
    "workflows": []
  },

  "budget": {
    "total_per_run": 7500,
    "cost_estimate": "$0.0043"
  }
}
```

### Activity vs. Step

These two terms are distinct and should never be conflated:

- **Activity** — A bounded unit of work within a workflow. Has its own `intent`, `skills`, `model`, and `token_budget`. Corresponds to a Runner.Run() call in the execution engine.
- **Step** — A single natural-language instruction inside an activity. Steps are what the agent reads and follows. They define the procedure.

A workflow has activities. An activity has steps.

### Model selection per activity

Each activity declares which model to use. This is not a heuristic — it is an explicit instruction to Janus. The agent does not select the model; the workflow designer does.

**Guidance for model selection:**

| Activity type | Recommended model |
|---------------|-------------------|
| Simple data retrieval (CRM lookup, file read) | haiku |
| Conditional logic, routing, action-taking | haiku |
| Nuanced judgment requiring domain expertise | sonnet |
| Complex multi-step reasoning with skill context | sonnet |
| Strategic analysis or synthesis | opus (rare, high cost) |

The goal is the cheapest model that reliably handles the activity. A Workflow Specialist's entire job is optimizing this.

### Token budgets

Each activity has a `token_budget.max`. The `budget.total_per_run` at the workflow level is the sum of all activity ceilings plus overhead. This makes per-run cost deterministic and allows marketplace pricing to be accurate.

**Token budget components per activity:**
- Skill context loaded for this activity (counted from SKILL.md file sizes)
- Step instructions passed to the agent
- Agent reasoning overhead

### Sub-workflows

An activity can invoke another workflow. The invoked workflow runs as a sub-agent and can be synchronous (wait for completion) or asynchronous (fire and continue).

For long-running processes that require waiting on external events (e.g., "wait for client reply"), decompose into separate workflows connected by event triggers at the Role level rather than building async wait state into a single workflow. This keeps each workflow bounded and token-predictable.

### Error handling

Each activity declares `on_error` behavior:

```json
"on_error": {
  "retry": 2,
  "fallback": "notify_owner"
}
```

- `retry` — number of retry attempts before fallback
- `fallback` options: `notify_owner`, `skip`, `abort`

Default if `on_error` is omitted: retry once, then notify owner.

### Installation

```go
InstallWorkflow(ctx, "WORK-XXXX-XXXX-XXXX")
```

Installing a workflow auto-installs all its declared dependencies — skills and sub-workflows. A standalone workflow without a Role can still be invoked manually or via the `work` tool, but it has no scheduled triggers. Only a Role binds a workflow to a schedule.

---


## 3. Roles

### What a Role is

A Role is a job description with a schedule. It bundles Workflows and Skills into a complete job profile — and it defines *when* each workflow runs. The Role is the only artifact type that owns event bindings. A Workflow is a pure procedure; the Role gives it a rhythm.

Think about a human Chief of Staff. When you hire one, you're not hiring someone who *can* do morning briefings. You're hiring someone who *knows when* to do them without being told. The timing is part of the job definition. That's what a Role is.

A Role is three files: `manifest.json` + `role.json` + `ROLE.md`.

### File structure

```
ROLE-XXXX-XXXX-XXXX/
├── manifest.json     — Universal identity envelope
├── role.json         — Job definition: workflow-to-trigger bindings, event schedule
└── ROLE.md           — Persona and job description (prose)
```

### When to use a Role

Use Roles to present a complete job function to a non-technical user. Instead of asking a realtor to browse and install five individual workflows, the marketplace presents a "Client Intake Specialist" Role that installs everything at once — procedures, knowledge, capabilities, and the schedule that ties them together.

The non-technical user's interaction is: enter a code, get an employee.

### role.json — The Job Definition

The `role.json` carries the operational structure: which workflows to run, when to run them, and what events to listen for. This is the file that makes a Role more than a folder of workflows — it's what makes it an employee who already knows the job.

```json
{
  "workflows": {
    "lead-qualification": {
      "ref": "WORK-A1B2-C3D4-E5F6",
      "trigger": {
        "type": "event",
        "sources": ["form.submitted"]
      },
      "description": "Qualifies incoming leads in real time"
    },
    "client-followup": {
      "ref": "WORK-G7H8-I9J0-K1L2",
      "trigger": {
        "type": "schedule",
        "cron": "0 9 * * 1-5"
      },
      "description": "Morning follow-up on pending leads"
    },
    "consultation-scheduler": {
      "ref": "WORK-M3N4-O5P6-Q7R8",
      "trigger": {
        "type": "event",
        "sources": ["lead.qualified"]
      },
      "description": "Books consultations for qualified leads"
    }
  },
  "skills": [
    "SKIL-sales-qualification",
    "SKIL-onboarding-tone",
    "SKIL-customer-service"
  ],
  "pricing": {
    "model": "monthly_fixed",
    "cost": 47.0
  },
  "defaults": {
    "timezone": "user_local",
    "configurable": [
      "workflows.client-followup.trigger.cron"
    ]
  }
}
```

#### Trigger types

| Type | Fields | Description |
|------|--------|-------------|
| `schedule` | `cron` (string) | Fires on a cron schedule. Standard 5-field cron expression. |
| `heartbeat` | `interval` (string), `window` (string, optional) | Fires at a recurring interval. Window limits active hours (e.g., `"08:00-18:00"`). |
| `event` | `sources` (string[]) | Fires when any listed event occurs. |
| `manual` | — | Only fires by explicit user request or API call. |

> **Key principle:** The workflow doesn't decide when it runs. The Role does. The same workflow can run at 7am in one Role and 9am in another. The procedure doesn't change just because you want your briefing at a different time.

#### User-configurable fields

The `defaults.configurable` array lists JSON paths within `role.json` that the user can override after installation. The Role ships with opinionated defaults; the user adjusts what matters to them without editing the workflow or role definition.

### ROLE.md — The Persona

The `ROLE.md` is the agent's job description in prose. It defines who the agent *is* when operating in this Role — personality, communication style, priorities, judgment calls. No YAML frontmatter — identity lives in `manifest.json`, operational wiring lives in `role.json`, and the persona is pure markdown.

```markdown
# Client Intake Specialist

You handle first impressions with new prospects. You qualify incoming leads,
gather their information, and route them to the right next step — automatically.

## Communication Style

- Professional but warm — these are potential clients, not data points
- When referring out, always provide alternatives and explain why
- Internal notifications are crisp and data-driven

## Judgment

- A qualified lead scores >= 7 on fit, budget signals, and service area
- Previously declined prospects are flagged, not re-qualified
- When in doubt, qualify up — false negatives cost more than false positives
```

### Role as marketplace entry point

The Role's install code (`ROLE-7K3M-Q9P2-X4W1`) is shareable by users directly. A realtor texts a code to another realtor. No browsing, no sign-up friction. The Nebo agent resolves the code, fetches the Role package from NeboLoop, and installs all declared workflows and their transitive dependencies.

### Auto-install cascade

When a user installs a Role:

1. Parse `role.json` for all workflow references
2. For each workflow: resolve and install its declared dependencies (skills, sub-workflows)
3. Install any additional skills listed directly in `role.json`
4. Register all trigger bindings from `role.json`
5. Load the ROLE.md persona into the agent's context

The user installs a job. Everything else cascades.

---

## 4. The Workflow Specialist

The Skills/Workflows/Roles taxonomy creates a new human job category that did not exist before Nebo: the **Workflow Specialist**.

### What a Workflow Specialist does

A Workflow Specialist designs, optimizes, and sells AI job roles on NeboLoop. Their work is to take a human job function and decompose it into workflows that an AI agent can execute reliably and cost-effectively.

Their specific craft includes:

- **Job decomposition** — breaking a role into discrete workflows, each workflow into activities, each activity into steps
- **Model selection** — choosing the cheapest model that reliably handles each activity
- **Token budget optimization** — running workflows repeatedly, measuring actual usage, tightening budgets without degrading quality
- **Skill curation** — loading only the skills needed for each activity, not globally
- **Cost modeling** — producing accurate `cost_estimate` values that translate into accurate marketplace pricing
- **Quality assurance** — testing workflows against edge cases, verifying the agent follows the steps correctly

### Economic incentive

A Workflow Specialist earns revenue share for every install of their Roles and Workflows on NeboLoop. Their incentive is direct: better optimization (lower cost at the same quality) = lower marketplace price = more installs = more revenue.

The marketplace listing reflects their optimization work explicitly:

```
Client Intake Specialist by @workflow_pro
5 workflows · avg 7,500 tokens/run · $0.02/run
Optimized: 70% haiku / 30% sonnet routing
4.8 (142 installs) · $47/month
```

---

## 5. The Platform Hierarchy

```
ROLE  >  WORK  >  SKILL
(job)   (procedure) (knowledge)
```

**Design direction:** Start with what the agent needs to *know* (Skill), chain knowledge into procedures (Workflow), and compose procedures into a job with a schedule (Role).

**Dependency direction:** Installing a Role cascades downward — installs Workflows, which install Skills.

**Trigger ownership:** Roles own the schedule. Workflows are pure procedures with no opinion about timing. The same workflow can run on different schedules in different Roles.

```
ROLE       — job with a schedule (owns triggers, binds workflows to events)
  └── WORK — pure procedure (activities, steps, budgets — no triggers)
        ├── SKILL — knowledge (loaded per activity)
        └── WORK  — sub-workflow (called per activity)

NEBO       — the agent that runs all of the above
LOOP       — the community where all of the above is shared
```

**Package envelope:** Every artifact type ships a `manifest.json` as its universal identity envelope. The store reads one file to index any package. Artifact-specific files carry domain logic only.

| Artifact | Package Contents |
|----------|-----------------|
| Skill | `manifest.json` + `SKILL.md` |
| Workflow | `manifest.json` + `workflow.json` + `WORKFLOW.md` |
| Role | `manifest.json` + `role.json` + `ROLE.md` |

Every object has a code. Every code maps to an SDK install method. Every install resolves dependencies transitively before execution begins.

The non-technical user's experience: enter a code, get a working AI employee.
The Workflow Specialist's experience: design, optimize, publish, earn.

---

## 6. Execution Model

Nebo's workflow engine is a single-node Temporal-style orchestrator where the worker is an AI agent. Existing infrastructure maps directly:

| Temporal concept | Nebo equivalent |
|------------------|-----------------|
| Workflow | `workflow.json` |
| Activity | activity object in `workflow.json` |
| Worker | Nebo agent runner |
| Task queue | Lane system (event, cron, main) |
| Retry policy | `on_error` per activity |
| Child workflow | sub-workflow reference |
| Workflow history | shared conversation session |
| Persistence | SQLite |

Each activity is a bounded `Runner.Run()` call. Activities within the same workflow share a conversation session, so context from the `lookup` activity is naturally visible to the `assess` activity. For v1, workflows are constrained to a single model provider family per workflow to ensure session compatibility.

Async waits (e.g., "wait for email reply") are handled by decomposing into separate workflows connected by event triggers at the Role level rather than persisting mid-workflow state. Workflow A sends the email and completes. The Role's `role.json` defines an event trigger that fires when the reply arrives. Workflow B handles the reply. Each workflow is short, bounded, and token-predictable. The Role orchestrates the timing; the workflows stay pure procedures.

---

## 7. Install Code Resolution

Every primitive in the Nebo ecosystem is identified by a typed install code. The code prefix determines how the agent resolves and installs the object.

### Code Format

All codes follow the pattern: `{PREFIX}-XXXX-XXXX-XXXX` where each segment is 4 alphanumeric characters (uppercase + digits).

### Resolution Pipeline

```
User enters code (e.g., "ROLE-7K3M-Q9P2-X4W1")
  -> Agent parses prefix
  -> Route to correct SDK method
  -> Fetch metadata from NeboLoop API
  -> Resolve transitive dependencies
  -> Install all dependencies first
  -> Install primary object
  -> Confirm to user
```

### Prefix Routing Table

| Prefix | SDK Method | What Gets Installed | Dependency Resolution |
|--------|-----------|--------------------|-----------------------|
| `SKIL-` | `InstallSkill(ctx, code)` | manifest.json + SKILL.md | None — skills are leaf nodes |
| `WORK-` | `InstallWorkflow(ctx, code)` | manifest.json + workflow.json + WORKFLOW.md | Auto-install all declared skills and sub-workflows |
| `ROLE-` | `InstallRole(ctx, code)` | manifest.json + role.json + ROLE.md + all declared workflows (transitive) | Auto-install all workflows + their full dependency trees, register trigger bindings |
| `LOOP-` | `JoinLoop(ctx, code)` | Loop membership + shared context | No binary install — joins community channel |
| `NEBO-` | Reserved | Agent instance identifier | NOT installable — used for routing and identity |

### Dependency Resolution Order

For a Role install, the resolution order is:

```
1. Fetch Role package from NeboLoop (manifest.json + role.json + ROLE.md)
2. Parse role.json for workflow references
3. For each workflow:
   a. Fetch workflow package (manifest.json + workflow.json + WORKFLOW.md)
   b. Parse dependencies.skills -> queue skill installs
   c. Parse dependencies.workflows -> recurse (sub-workflows)
4. Install additional skills listed directly in role.json
5. Deduplicate all queued installs
6. Install in order: skills -> workflows -> register trigger bindings -> load persona
```

### Conflict Resolution

- **Circular dependencies:** Detected during marketplace submission (rejected). The resolution algorithm tracks visited nodes and aborts on cycles.

### Code Sharing

Install codes are designed for human sharing -- text messages, emails, social media. The alphanumeric format avoids ambiguous characters (no 0/O, 1/l confusion in the 4-char segments). Codes are case-insensitive for input but stored uppercase.

---

## 8. Marketplace Integration

The NeboLoop marketplace is the distribution channel for all primitives. Every install code resolves through NeboLoop's REST API.

### Store API Endpoints

```
GET  /api/v1/store/apps                    -> List apps (search, category, pagination)
GET  /api/v1/store/apps/{id}               -> App detail (metadata, screenshots, reviews)
GET  /api/v1/store/apps/{id}/reviews       -> App reviews (ratings, distribution)
POST /api/v1/store/apps/{id}/install       -> Install app (triggers background download)
DELETE /api/v1/store/apps/{id}/install     -> Uninstall app

GET  /api/v1/store/skills                  -> List skills (search, category, pagination)
POST /api/v1/store/skills/{id}/install     -> Install skill (fetches SKILL.md)
DELETE /api/v1/store/skills/{id}/install   -> Uninstall skill

GET  /api/v1/store/workflows               -> List workflows (future)
GET  /api/v1/store/roles                   -> List roles (future)
```

### Revenue Model

| Primitive | Pricing Model | Revenue Split |
|-----------|--------------|---------------|
| Skill | Free | N/A |
| Workflow | Per-run or subscription | 70% creator / 30% NeboLoop |
| Role | Monthly subscription | 70% creator / 30% NeboLoop |

Skills are always free -- they are pure knowledge with negligible distribution cost. Workflows and roles are monetizable. The 70/30 split is applied after payment processing fees.

### Submission Pipeline

```
Developer submits primitive
  -> Automated checks (manifest validation, signing, size limits)
  -> Dependency resolution test (all declared deps must exist)
  -> Security scan (static analysis of binaries)
  -> Review queue (manual review for first submission, auto-approve for trusted publishers)
  -> Published to marketplace
```

### Install Tracking

Every install is tracked in both NeboLoop (server-side) and Nebo (client-side):

- **NeboLoop:** `installs` table tracks user_id, primitive_id, installed_at, version
- **Nebo:** `plugin_registry` table tracks id, type, source, installed_at, version, status

The `plugin_registry` is the local source of truth. NeboLoop is the authoritative source for available versions and updates.

### Update Flow

```
NeboLoop sends update_available event via WebSocket
  -> Nebo checks local plugin_registry version
  -> If newer version: background download
  -> For skills/workflows: auto-apply (no binary, no permissions)
```

---

## 9. Rust Implementation Status

| Component | Go | Rust | Notes |
|-----------|-----|------|-------|
| SKILL.md loading | Y | Y | `crates/agent/src/skills/` — fsnotify hot-reload |
| Skill trigger matching | Y | Y | Case-insensitive substring matching |
| Skill execution (context injection) | Y | Y | Injected into system prompt per session |
| Skill store install | Y | N | Store routes NOT implemented in Rust |
| Universal manifest.json | N | N | New feature — universal envelope for all artifact types |
| Workflow engine | N | N | New feature — spec in `workflow-engine.md` |
| Role install (role.json) | N | N | New feature — includes trigger binding registration |
| Role trigger registration | N | N | New feature — Roles own event bindings, workflows are triggerless |
| Install code resolution | P | N | Go has store install but NOT typed prefix routing |
| Dependency resolution (transitive) | N | N | New feature — designed for Rust |
| Marketplace store API | Y | N | Go has full store handlers, Rust has none |
| Update flow (WS events) | Y | P | Rust has updater for binary, NOT for skills |
| Revenue/billing integration | N/A | N/A | Server-side (NeboLoop), NOT in Nebo client |

**Legend:** Y = Implemented, N = Not implemented, P = Partially implemented

---

*This document captures decisions from discussions 4743279a-d5df-4629-a7bc-622ef7a9dc85 and subsequent architecture sessions. Key changes: manifest.json is now a universal package envelope for all artifact types; triggers moved from workflow.json to role.json; Roles own the schedule, Workflows are pure procedures.*

*Last updated: 2026-03-06*
