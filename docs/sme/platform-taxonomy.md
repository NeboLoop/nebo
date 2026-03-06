# Nebo Platform Taxonomy: Skills, Tools, Workflows, and Roles

**Document Type:** SME Architecture Reference
**Status:** Canonical
**Discussion Reference:** 4743279a-d5df-4629-a7bc-622ef7a9dc85

---

## Overview

Nebo's platform is built around five primitive objects, each with a unique install code and a defined role in the hierarchy:

| Code Prefix | Primitive | What It Is |
|-------------|-----------|------------|
| `SKILL-XXXX-XXXX-XXXX` | Skill | Knowledge the agent loads into context |
| `TOOL-XXXX-XXXX-XXXX` | Tool | Executable capability the agent invokes |
| `WORK-XXXX-XXXX-XXXX` | Workflow | A procedure composed of activities |
| `NEBO-XXXX-XXXX-XXXX` | Agent (Nebo) | The agent instance that runs everything |
| `LOOP-XXXX-XXXX-XXXX` | Community | The NeboLoop community where primitives are shared |

A **Role** is a marketplace concept — a curated bundle of Workflows presented as a job function. It is not a system primitive with its own runtime object. See the Roles section for the `ROLE.md` format.

---

## 1. Skills

### What a Skill is

A Skill is pure knowledge. It is training, expertise, and domain context that an agent loads into its working memory when it needs to reason well about a specific type of task. A Skill does not execute anything. It does not call APIs. It makes the agent smarter.

A Skill is **a single file**: `SKILL.md`.

### When Skills are used

Skills are injected into the agent's context at the point where they are needed — either when a Tool declares it needs them to operate, or when a Workflow activity specifies them for a particular step. The agent reads the Skill as part of its context window and uses that knowledge to inform its reasoning and output quality.

### File structure

```
SKILL-XXXX-XXXX-XXXX/
└── SKILL.md
```

The `SKILL.md` is plain markdown. It can include:

- Domain knowledge (e.g., "here is how to score a sales lead")
- Tone and style guides (e.g., "here is how we communicate with clients")
- Rules and heuristics (e.g., "here are the criteria for qualifying a prospect")
- Procedural knowledge (e.g., "here is how to handle an edge case in tax law")

### Examples

- `SKILL-sales-qualification` — How to evaluate whether a lead is worth pursuing
- `SKILL-onboarding-tone` — How to write warm, professional onboarding communications
- `SKILL-tax-law-schedule-c` — Current tax code guidance for Schedule C filers
- `SKILL-industry-knowledge-realestate` — Real estate market context and terminology

### Installation

```go
InstallSkill(ctx, "SKILL-XXXX-XXXX-XXXX")
```

Skills are free, lightweight, and always auto-installed when a Tool or Workflow depends on them. There is no reason to present the user with a choice about Skill installation.

---

## 2. Tools

### What a Tool is

A Tool is executable capability. It is something the agent can do: call an external API, read a file, send an email, query a database, run a computation, post to Slack. Tools are the agent's hands.

A Tool is three files: `SKILL.md` + `manifest.json` + an executable.

### File structure

```
TOOL-XXXX-XXXX-XXXX/
├── SKILL.md          — What this tool does, how to use it, when to use it
├── manifest.json     — Metadata, parameters, return types, interface declarations
└── <executable>      — Go binary, script, gRPC server, MCP endpoint, etc.
```

The `SKILL.md` in a Tool is the agent's user manual for the tool. It describes what the tool does, what inputs it expects, what it returns, and how to invoke it effectively. The agent reads this to understand when and how to use the tool.

### manifest.json

```json
{
  "id": "TOOL-A1B2-C3D4-E5F6",
  "name": "AcmeCRM",
  "version": "1.0.0",
  "author": "acme",
  "description": "Query and update Acme CRM contacts and opportunities",
  "implements": ["crm-lookup", "contact-search", "contact-update"],
  "parameters": {
    "query": { "type": "string", "required": true },
    "limit": { "type": "integer", "required": false, "default": 10 }
  },
  "returns": {
    "contacts": { "type": "array" },
    "total": { "type": "integer" }
  }
}
```

The `implements` field is critical. It declares which **tool interfaces** this tool satisfies. Tool interfaces are abstract capability declarations (e.g., `crm-lookup`, `email-send`) that Workflows reference when they need a capability but want to remain tool-agnostic. NeboLoop indexes these at install time for dependency resolution.

### Tool binding modes

Workflows can reference tools in two ways:

**Interface binding** (generic, swappable):
```json
{ "interface": "crm-lookup" }
```
The workflow says "I need a CRM lookup capability." At install time, the user chooses which installed tool satisfies this interface. If they have multiple options, they pick. If they have none, the marketplace guides them to compatible tools.

**Code binding** (pinned, specific):
```json
{ "code": "TOOL-A1B2-C3D4-E5F6" }
```
The workflow requires exactly this tool. Used when a tool developer builds workflows optimized specifically for their own tool's behavior and API response format.

### MCP Tools

An MCP server endpoint is a Tool. The `manifest.json` declares the MCP URL and the `implements` interfaces it satisfies. The executable is the MCP server. The pattern is identical.

### Examples

- `TOOL-crm-lookup` — Query Salesforce, HubSpot, or a custom CRM
- `TOOL-email-send` — Send email via Gmail, Outlook, or Outlet
- `TOOL-calendar-manage` — Read availability and book slots in Google Calendar
- `TOOL-slack-notify` — Post messages to Slack channels

### Installation

```go
InstallApp(ctx, "TOOL-XXXX-XXXX-XXXX")
```

---

## 3. Workflows

### What a Workflow is

A Workflow is a procedure. It defines the repeatable steps required to accomplish a specific job function — decomposed into **activities**, each with its own intent, skills, tools, model selection, and token budget.

A Workflow is three files: `SKILL.md` + `manifest.json` + `workflow.json`.

### File structure

```
WORK-XXXX-XXXX-XXXX/
├── SKILL.md          — What this workflow does, when to run it, how the agent should approach it
├── manifest.json     — Metadata, versioning, dependencies
└── workflow.json     — The orchestration definition
```

### The four layers of a Workflow

A well-designed workflow gives the agent everything it needs to do a job reliably and intelligently:

1. **Specialized knowledge** — Skills loaded per activity give the agent domain expertise
2. **Executable capabilities** — Tools available per activity give the agent the ability to take action
3. **Repeatable procedure** — Steps within each activity define what to do, in order, every time
4. **Autonomy** — The agent's intent-driven reasoning means it handles edge cases intelligently without rigid hardwiring

### workflow.json schema

```json
{
  "version": "1.0",
  "id": "lead-qualification",
  "name": "Lead Qualification",

  "triggers": [
    { "type": "event", "event": "new_contact_form_submission" },
    { "type": "schedule", "cron": "0 9 * * 1-5" },
    { "type": "manual" }
  ],

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
      "tools": ["TOOL-A1B2-C3D4-E5F6"],
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
      "skills": ["SKILL-sales-qualification", "SKILL-industry-knowledge"],
      "tools": ["TOOL-A1B2-C3D4-E5F6"],
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
      "skills": ["SKILL-onboarding-tone"],
      "tools": ["TOOL-A1B2-C3D4-E5F6", "TOOL-G7H8-I9J0-K1L2"],
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
    "skills": ["SKILL-sales-qualification", "SKILL-industry-knowledge", "SKILL-onboarding-tone"],
    "tools": [
      { "code": "TOOL-A1B2-C3D4-E5F6", "name": "AcmeCRM" },
      { "code": "TOOL-G7H8-I9J0-K1L2", "name": "AcmeMail" }
    ],
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

- **Activity** — A bounded unit of work within a workflow. Has its own `intent`, `skills`, `tools`, `model`, and `token_budget`. Corresponds to a Runner.Run() call in the execution engine.
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
- Agent reasoning and tool call overhead
- Tool response parsing

### Triggers

Workflows are triggered by events, schedules, or manual invocation. The trigger system maps to existing Nebo infrastructure:

- `event` → event lane (fires on a named system or user event)
- `schedule` → cron lane (standard cron expression)
- `manual` → direct invocation by user or another workflow

### Sub-workflows

An activity can invoke another workflow. The invoked workflow runs as a sub-agent and can be synchronous (wait for completion) or asynchronous (fire and continue).

For long-running processes that require waiting on external events (e.g., "wait for client reply"), decompose into separate workflows connected by triggers rather than building async wait state into a single workflow. This keeps each workflow bounded and token-predictable.

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

Installing a workflow auto-installs all its declared dependencies — skills, tools, and sub-workflows — before the first activity runs.

---

## 4. Roles

### What a Role is

A Role is a job description. It bundles a set of Workflows that together replicate a human job function. A Role is not a system primitive — there is no `InstallRole()` runtime method in v1. A Role is a marketplace packaging concept.

A Role is a single file: `ROLE.md` with YAML frontmatter.

### When to use a Role

Use Roles to present a complete job function to a non-technical user. Instead of asking a realtor to browse and install five individual workflows, the marketplace presents a "Client Intake Specialist" Role that installs everything at once.

The non-technical user's interaction is: enter a code, get an employee.

### ROLE.md format

```markdown
---
id: client-intake-specialist
name: Client Intake Specialist
code: ROLE-7K3M-Q9P2-X4W1
description: Handles new client inquiries, qualifies leads, and schedules initial consultations
workflows:
  - WORK-A1B2-C3D4-E5F6
  - WORK-G7H8-I9J0-K1L2
  - WORK-M3N4-O5P6-Q7R8
tools:
  - TOOL-A1B2-C3D4-E5F6
  - TOOL-G7H8-I9J0-K1L2
skills:
  - SKILL-sales-qualification
  - SKILL-onboarding-tone
  - SKILL-customer-service
pricing:
  model: monthly_fixed
  amount: 47
  currency: usd
estimated_tokens_per_day: 45000
estimated_cost_per_run: "$0.02"
---

# Client Intake Specialist

A Client Intake Specialist handles your first impression with new prospects.
This role qualifies incoming leads, gathers their information, and routes them
to the right next step — automatically.

## What it does

- Receives new contact form submissions in real time
- Looks up existing records in your CRM to identify prior relationships
- Scores leads based on fit, budget signals, and service area
- Routes qualified leads to your sales team with a summary
- Sends polite referral emails to non-fits with suggested alternatives

## What it needs

Three workflows that run on triggers:

1. **Lead Qualification** — runs when a new form is submitted
2. **Client Follow-Up** — runs on a morning schedule for pending leads
3. **Consultation Scheduler** — runs when a qualified lead is ready to book

These workflows collectively require a CRM tool and an email tool.
You will be prompted to connect compatible tools during installation.

## Performance

- Estimated token usage: ~7,500 per lead qualified
- Estimated cost per lead: $0.02
- Monthly pricing: $47 (assumes up to ~2,350 qualified interactions/month)
```

### Role as marketplace entry point

The Role's install code (`ROLE-7K3M-Q9P2-X4W1`) is shareable by users directly. A realtor texts a code to another realtor. No browsing, no sign-up friction. The Nebo agent resolves the code, fetches the `ROLE.md` from NeboLoop, and installs all declared workflows and their transitive dependencies.

---

## 5. The Workflow Specialist

The Skills/Tools/Workflows/Roles taxonomy creates a new human job category that did not exist before Nebo: the **Workflow Specialist**.

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

### Workflow Specialists vs. Tool Developers

These are distinct roles with complementary incentives:

| | Tool Developer | Workflow Specialist |
|---|---|---|
| **Builds** | Tools with specific capabilities | Workflows that use those tools |
| **Sells** | Tool installs | Workflow/Role installs |
| **Incentive** | More tools used = more revenue | More workflows sold = more revenue |
| **Expertise** | API integration, Go/MCP development | Job process design, LLM optimization |

A Tool Developer can also be a Workflow Specialist — they build a CRM tool and then build the best-possible workflows for it, because nobody knows their tool's capabilities and response formats better than they do.

---

## 6. The Platform Hierarchy

```
ROLE       — job function (marketplace bundle)
  └── WORK — workflow (procedure)
        ├── SKILL — knowledge (loaded per activity)
        ├── TOOL  — capability (invoked per activity)
        └── WORK  — sub-workflow (called per activity)

NEBO       — the agent that runs all of the above
LOOP       — the community where all of the above is shared
```

Every object has a code. Every code maps to an SDK install method. Every install resolves dependencies transitively before execution begins.

The non-technical user's experience: enter a code, get a working AI employee.
The Workflow Specialist's experience: design, optimize, publish, earn.
The Tool Developer's experience: build a capability, the ecosystem builds on top of it.

---

## 7. Execution Model

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

Async waits (e.g., "wait for email reply") are handled by decomposing into separate trigger-linked workflows rather than persisting mid-workflow state. Workflow A sends the email and completes. An event trigger fires when the reply arrives. Workflow B handles the reply. Each workflow is short, bounded, and token-predictable.

---

## 8. Install Code Resolution

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
| `SKILL-` | `InstallSkill(ctx, code)` | Single SKILL.md file | None -- skills are leaf nodes |
| `TOOL-` | `InstallApp(ctx, code)` | manifest.json + binary + SKILL.md | Auto-install declared skill dependencies |
| `WORK-` | `InstallWorkflow(ctx, code)` | workflow.json + manifest.json + SKILL.md | Auto-install all declared skills, tools, and sub-workflows |
| `ROLE-` | `InstallRole(ctx, code)` | All declared workflows (transitive) | Auto-install all workflows + their full dependency trees |
| `LOOP-` | `JoinLoop(ctx, code)` | Loop membership + shared context | No binary install -- joins community channel |
| `NEBO-` | Reserved | Agent instance identifier | NOT installable -- used for routing and identity |

### Dependency Resolution Order

For a Role install, the resolution order is:

```
1. Fetch ROLE.md from NeboLoop
2. Parse workflow list from frontmatter
3. For each workflow:
   a. Fetch workflow.json
   b. Parse dependencies.skills -> queue skill installs
   c. Parse dependencies.tools -> queue tool installs
   d. Parse dependencies.workflows -> recurse (sub-workflows)
4. Deduplicate all queued installs
5. Install in order: skills -> tools -> workflows -> register role
```

### Conflict Resolution

- **Version conflicts:** If two workflows depend on different versions of the same tool, the HIGHER version wins. The lower-version workflow is tested against the higher version during marketplace submission.
- **Interface conflicts:** If two tools implement the same interface, the user chooses during install. The selection is persisted in `plugin_settings`.
- **Circular dependencies:** Detected during marketplace submission (rejected). The resolution algorithm tracks visited nodes and aborts on cycles.

### Code Sharing

Install codes are designed for human sharing -- text messages, emails, social media. The alphanumeric format avoids ambiguous characters (no 0/O, 1/l confusion in the 4-char segments). Codes are case-insensitive for input but stored uppercase.

---

## 9. Marketplace Integration

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
| Tool | One-time or subscription | 70% creator / 30% NeboLoop |
| Workflow | Per-run or subscription | 70% creator / 30% NeboLoop |
| Role | Monthly subscription | 70% creator / 30% NeboLoop |

Skills are always free -- they are pure knowledge with negligible distribution cost. Tools, workflows, and roles are monetizable. The 70/30 split is applied after payment processing fees.

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
  -> For tools with new permissions: stage in .pending/, require user approval
  -> For tools with no new permissions: auto-apply
  -> For skills/workflows: auto-apply (no binary, no permissions)
```

---

## 10. Rust Implementation Status

| Component | Go | Rust | Notes |
|-----------|-----|------|-------|
| SKILL.md loading | Y | Y | `crates/agent/src/skills/` -- fsnotify hot-reload |
| Skill trigger matching | Y | Y | Case-insensitive substring matching |
| Skill execution (context injection) | Y | Y | Injected into system prompt per session |
| Skill store install | Y | N | Store routes NOT implemented in Rust |
| Tool (app) install | Y | P | Basic manifest + binary, missing gRPC adapters |
| Tool interface binding | N | N | New feature -- NOT in Go, designed for Rust |
| Workflow engine | N | N | New feature -- spec in `workflow-engine.md` |
| Role install | N | N | New feature -- marketplace packaging only |
| Install code resolution | P | N | Go has store install but NOT typed prefix routing |
| Dependency resolution (transitive) | N | N | New feature -- designed for Rust |
| Marketplace store API | Y | N | Go has full store handlers, Rust has none |
| Plugin registry (local tracking) | Y | N | Go has `plugin_registry` table, Rust missing |
| Update flow (WS events) | Y | P | Rust has updater for binary, NOT for apps/skills |
| Revenue/billing integration | N/A | N/A | Server-side (NeboLoop), NOT in Nebo client |

**Legend:** Y = Implemented, N = Not implemented, P = Partially implemented

---

*This document captures decisions made in discussion 4743279a-d5df-4629-a7bc-622ef7a9dc85 with input from the Nebo SME, NeboLoop SME, and Rust shell agents.*

*Generated: 2026-03-04*
