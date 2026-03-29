# Agent Orchestration, Pipelines & Enterprise Architecture — SME Reference

> Strategic and technical design document covering Commander orchestration,
> isolated agent sessions, concurrent agent instances, and the service/pod
> pipeline model. Establishes Nebo as an enterprise-grade intelligent
> automation platform.

**Created:** 2026-03-29  
**Status:** Design — Pre-Implementation

---

## Table of Contents

1. [Strategic Context](#1-strategic-context)
2. [Architecture Overview](#2-architecture-overview)
3. [Capability 1 — Commander: Directed Agent Orchestration](#3-capability-1--commander-directed-agent-orchestration)
4. [Capability 2 — Isolated Agent Sessions](#4-capability-2--isolated-agent-sessions)
5. [Capability 3 — Concurrent Agent Instances](#5-capability-3--concurrent-agent-instances)
6. [Capability 4 — Service/Pod Model & Processing Pipelines](#6-capability-4--servicepod-model--processing-pipelines)
7. [Intelligence at Every Layer](#7-intelligence-at-every-layer)
8. [Enterprise Positioning](#8-enterprise-positioning)
9. [Rename: ROLE → AGENT](#9-rename-role--agent)
10. [What Exists vs. What Needs Building](#10-what-exists-vs-what-needs-building)
11. [Key Files Reference](#11-key-files-reference)

---

## 1. Strategic Context

Nebo's agent architecture — combined with the service/pod pipeline model — is
not a chat assistant with plugins. It is an **enterprise-grade intelligent
automation platform** where every processing node has reasoning capability.

This makes a large category of SaaS products obsolete. Traditional SaaS
automates fixed processes with dumb functions. Nebo automates adaptive processes
with intelligent agents. The difference is not incremental — it is architectural.

**Examples of displacement:**

| Traditional SaaS | Nebo Equivalent |
|---|---|
| Document management + OCR | Document Processing Agent pipeline |
| Email routing & ticketing | Email Triage Agent pipeline |
| CRM data enrichment | Lead Enrichment Agent service |
| Content workflow tools | Content Creation Agent pipeline |
| Contract review software | Contract Analysis Agent pipeline |
| Ad production platforms | Ad Creation Agent pipeline |
| Invoice processing tools | Finance Document Agent service |

The mechanism of displacement: every stage in a Nebo pipeline is an agent that
**reasons, uses tools, applies memory, and handles exceptions** — capabilities
that dumb workflow engines and fixed-function SaaS cannot match.

The marketplace becomes an enterprise app store for agent services and pipelines.
Publishers ship not just individual agents but entire vertical pipeline packages.

---

## 2. Architecture Overview

Two complementary dispatch models coexist:

```
┌─────────────────────────────────────────────────────────────────┐
│                        NEBO PLATFORM                            │
│                                                                 │
│  ┌─────────────┐     COMMANDER MODEL (deliberate routing)       │
│  │Primary Agent│ ──→ Travel Agent    (isolated session)         │
│  │             │ ──→ Research Agent  (isolated session)         │
│  │             │ ──→ PI Agent        (isolated session)         │
│  └─────────────┘                                                │
│                                                                 │
│  ┌─────────────────────┐  SERVICE MODEL (throughput routing)    │
│  │  Document Service   │ ──→ Doc Processor Instance A           │
│  │  (work queue)       │ ──→ Doc Processor Instance B           │
│  │                     │ ──→ Doc Processor Instance C           │
│  └─────────────────────┘                                        │
│                                                                 │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │               PIPELINE (chained services)                │   │
│  │                                                          │   │
│  │  Email In → [Triage] → [Route] → [Draft] → [Review]     │   │
│  │                  ↓                                       │   │
│  │             [Invoice] → [Extract] → [Accounting]         │   │
│  │                                                          │   │
│  │  Every [ ] is an agent service with intelligent workers  │   │
│  └──────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

---

## 3. Capability 1 — Commander: Directed Agent Orchestration

### What It Is

Commander is the deliberate routing layer. A primary agent **decides** to
delegate a specific task to a specific named agent. The caller has intent about
*who* handles the work.

Example: "Plan a trip to XYZ, research everything there is to do, find the best
travel dates, plan for 7 people." Primary agent dispatches to Travel Agent and
Research Agent simultaneously, waits for both to complete, synthesizes.

### The Gap Today

The `Orchestrator` in `crates/agent/src/orchestrator.rs` spawns anonymous
runners using `system_prompt_for_type()` (Explore/Plan/General). It has no
concept of agent identity. The `commander_teams` / `commander_edges` DB schema
exists (migration `0066`) but is not wired into dispatch.

### Design

**Dispatch flow:**

1. Primary agent calls Commander tool with target agent name + task payload
2. Commander resolves agent by name/id from `commander_team_members` → `agents` table
3. Pulls `agent_md` as the system prompt (not generic explore/plan/general)
4. Loads agent's configured skills, MCPs, tool permissions from `agent_workflows` / `entity_config`
5. Derives session key: `agent:{agent_id}` (isolated, persistent)
6. Dispatches via existing `Orchestrator.spawn()` with `wait: false` (non-blocking)
7. Stores `pending_task` with `parent_task_id` linking back to primary's current task
8. Primary receives results via `pending_tasks.output` + WebSocket notification when children complete

**What `commander_edges` enforces:**
- Which agents can talk to which — prevents arbitrary cross-agent calls
- `edge_type` can encode relationship semantics (reports_to, delegates_to, coordinates_with)
- Primary agent only sees agents it has edges to in its Commander tool

**Async flow:**
```
Primary dispatches Travel + Research (non-blocking)
    ├── Travel Agent runs in agent:{travel_id} session
    └── Research Agent runs in agent:{research_id} session
           ↓ (both complete independently)
Primary receives task_complete notifications via WebSocket
Primary synthesizes both results → responds to user
```

---

## 4. Capability 2 — Isolated Agent Sessions

### What It Is

Each agent maintains a fully isolated context bubble. No memory, no message
history, no context bleeds between agents. The PI Agent never sees what the
Travel Agent knows.

### The Problem Solved

Shared context caused conflation: PI subject details bled into travel planning,
research from one subject contaminated another. Isolation is now a first-class
architectural constraint, not a convention.

### Design

Session keys are namespaced by agent identity as an enforced rule at the
dispatch layer:

| Agent Type | Session Key Pattern | Memory | Lifetime |
|---|---|---|---|
| Primary agent | `user:{user_id}:primary` | Persistent | User lifetime |
| Specialist agent | `agent:{agent_id}` | Persistent | Agent lifetime |
| Concurrent instance | `agent:{agent_id}:instance:{ulid}` | Suppressed | Job lifetime |

**Enforcement:** When Commander routes to any agent, it **always** constructs
the session key from the agent's own ID. It never passes the primary agent's
session key to a specialist. This is enforced in the dispatch layer, not by
convention.

**Memory isolation:** `memory.rs` extraction already runs per session key. By
isolating session keys per agent, memory isolation is automatic. The PI Agent's
memories are scoped to `agent:{pi_agent_id}` and never visible to any other
session.

**Session mode toggle** (new config on `agents` table or `entity_config`):
- `persistent` (default) — one session per agent, accumulates memory over time
- `concurrent` — each job gets an isolated ephemeral instance session

---

## 5. Capability 3 — Concurrent Agent Instances

### What It Is

Multiple instances of the same agent type running in parallel, each with 100%
isolated context. The Document Processor agent can run as 12 simultaneous
instances, each processing a different document with no cross-contamination.

### Design

When `session_mode = concurrent`, each dispatch generates a unique instance ID
(ULID — ordered + unique). Session key: `agent:{agent_id}:instance:{ulid}`.

**Properties:**
- Each instance gets its own message history — no cross-contamination
- Memory extraction suppressed — instances are ephemeral, not persistent agents
- Instance session cleaned up after task completes (configurable: retain for audit or discard)
- CancellationToken cascade — cancelling the parent job cancels all instances

**Fan-out:** Commander tool gains a `concurrency` parameter. When a primary
agent dispatches a batch job to a concurrent-mode agent, Commander fans out via
existing `spawn_parallel()` — which already handles `FuturesUnordered` +
progress events. Each `SpawnRequest` gets its own instance session key.

**Tracking:** `pending_tasks.parent_task_id` already exists for batch tracking.
When all children complete, primary receives aggregated `SpawnResult.output`
(already implemented in `spawn_parallel_internal`).

**New behavior needed:** Commander tool detects `session_mode = concurrent` and
fans out automatically when given a list of inputs, rather than requiring the
caller to explicitly request parallelism.

---

## 6. Capability 4 — Service/Pod Model & Processing Pipelines

### What It Is

A load-balanced intake model where work arrives at a named **service endpoint**
and is claimed by the next available **agent instance** (pod). The caller
doesn't know or care which instance handles it. Analogous to Kubernetes
Service → Pod routing.

This is fundamentally different from Commander:
- **Commander:** "I want *that specific agent* to handle this task"
- **Service:** "I want *any available agent* of this type to handle this item"

### Service/Pod Model

```
Document Processor Service
├── Config: agent_type=document-processor, min=1, max=10
├── Work Queue (durable, ordered)
│     ├── Item: contract_001.pdf [pending]
│     ├── Item: contract_002.pdf [claimed by instance-B]
│     └── Item: contract_003.pdf [claimed by instance-A]
└── Instance Pool
      ├── Instance A → processing contract_003.pdf (session: agent:{id}:instance:{ulid-A})
      ├── Instance B → processing contract_002.pdf (session: agent:{id}:instance:{ulid-B})
      └── Instance C → idle, waiting for work
```

**Key properties:**
- Work queue is durable — survives restarts, tracked in `pending_tasks`
- Instances are stateless between jobs — context discarded after each item
- Caller submits to the service, not to an instance — decoupled
- Auto-scaling: spawn new instances when queue depth exceeds threshold, drain
  idle instances when queue is empty
- Back-pressure: configurable behavior when queue is full (drop, block, or
  reject with error)

**Service manifest** (extends agent config):
```json
{
  "service": {
    "enabled": true,
    "min_instances": 1,
    "max_instances": 10,
    "queue_depth_scale_threshold": 5,
    "idle_timeout_seconds": 300,
    "result_retention": "discard"
  }
}
```

### Processing Pipelines

Pipelines chain services together. The output of one service becomes the input
of the next, routed via the existing EventBus emit/subscribe system.

```
Email arrives (external event)
    │
    ▼
[Email Triage Service]     ← Agent reads email, classifies intent
    │
    ├─→ emit: "email.urgent"
    │       ▼
    │   [Response Draft Service]  ← Agent drafts reply using context
    │       ▼
    │   [Send Queue Service]      ← Agent reviews, sends or escalates
    │
    ├─→ emit: "email.invoice"
    │       ▼
    │   [Document Processing Service]  ← Agent extracts invoice data
    │       ▼
    │   [Accounting Agent]             ← Agent reconciles, posts entry
    │
    └─→ emit: "email.lead"
            ▼
        [CRM Enrichment Service]   ← Agent researches lead
            ▼
        [Sales Agent]              ← Agent personalizes outreach
```

**Pipeline definition** — declarative YAML alongside agent/service definitions:
```yaml
pipeline: email-management
stages:
  - service: email-triage
    on_emit:
      email.urgent: response-draft
      email.invoice: document-processing
      email.lead: crm-enrichment
  - service: response-draft
    next: send-queue
  - service: document-processing
    next: accounting-agent
  - service: crm-enrichment
    next: sales-agent
```

**Routing:** Stage transitions use the existing EventBus. Each service's agent
instances emit completion events with their output as payload. The EventDispatcher
routes to the next stage's service queue.

**This is already structurally supported** by the emit/subscribe system in
`crates/workflow/src/events.rs`. Pipeline definitions are a layer of
configuration on top of existing infrastructure.

---

## 7. Intelligence at Every Layer

This is the fundamental differentiator from traditional pipeline tools.

In conventional pipelines, stages are **dumb functions** — they transform data
according to fixed rules. Every edge case must be anticipated at design time and
encoded explicitly. Exceptions cause failures.

In Nebo pipelines, every stage is an **agent** with:

**Reasoning** — stages understand their input, not just pattern-match it. Email
triage doesn't classify by subject line keywords — it reads and comprehends the
email, handles novel cases, applies judgment.

**Tool access** — stages can take action, not just pass data. An enrichment
stage can research a lead in real time, call APIs, read documents, update
records.

**Memory** — persistent service agents accumulate knowledge over time. A
contract processor that handles the same client's contracts for months learns
their patterns, clause preferences, standard exceptions.

**Adaptive routing** — agents can deviate from the default next stage based on
what they find. A document processor that discovers a contract needs legal
review can emit a different event than the standard completion event.

**Intelligent failure** — when a stage can't handle something, it explains why,
attempts alternatives, and hands off with full context. No silent failures, no
crashes — reasoned escalation.

**Compound effect:** A 5-stage pipeline where each agent can reason around
problems, recover from ambiguity, and make judgment calls produces dramatically
better outcomes than the same pipeline with dumb functions. The system gets
smarter the more it runs — each persistent agent accumulates memory, each
pipeline accumulates calibration.

---

## 8. Enterprise Positioning

### What Makes a System Enterprise-Grade

| Requirement | Nebo Capability |
|---|---|
| Handle complex, multi-step processes | ✓ Pipeline architecture |
| Scale with volume | ✓ Service/pod auto-scaling |
| Auditable outcomes | ✓ Every agent decision is traceable |
| Handle exceptions gracefully | ✓ Agent reasoning at every stage |
| Composable from reusable components | ✓ Marketplace agent services |
| Integrate with existing tech stack | ✓ MCP + Commander routing |
| Adaptive to process variation | ✓ Intelligence at every layer |

### Comparable Systems

Nebo's pipeline architecture occupies the same category as:
- **MuleSoft** — enterprise integration and workflow orchestration
- **Temporal** — durable workflow execution
- **UiPath** — robotic process automation

But with a critical difference: those systems move **data** between **dumb
workers**. Nebo moves **context** between **intelligent agents**. The gap is not
incremental — it is architectural.

### The Marketplace as Enterprise App Store

Publishers ship:
- Individual agents (current)
- Agent services with queue management (new)
- Complete pipeline packages (new) — "Email Management Suite", "Contract
  Review Pipeline", "Lead Enrichment System"

Enterprises install a pipeline package, configure it to their environment, and
have an intelligent automation system running in minutes rather than months of
integration work.

---

## 9. Rename: ROLE → AGENT

All `role`/`Role`/`ROLE` references throughout the codebase are being renamed
to `agent`/`Agent`/`AGENT`. Migration `0070` handled the DB layer. The
following surfaces remain:

### Files to Rename

| Current | New |
|---|---|
| `crates/agent/src/role_worker.rs` | `crates/agent/src/agent_worker.rs` |
| `crates/agent/src/strap/role.txt` | `crates/agent/src/strap/agent.txt` |
| `crates/db/src/queries/roles.rs` | `crates/db/src/queries/agents.rs` |
| `crates/napp/src/role.rs` | `crates/napp/src/agent.rs` |
| `crates/napp/src/role_loader.rs` | `crates/napp/src/agent_loader.rs` |
| `crates/server/src/handlers/roles.rs` | `crates/server/src/handlers/agents.rs` |
| `crates/tools/src/role_tool.rs` | `crates/tools/src/agent_tool.rs` |
| `docs/publishers-guide/roles.md` | `docs/publishers-guide/agents.md` |
| `docs/sme/ROLES_SME.md` | `docs/sme/AGENTS_SME.md` |
| `tests/fixtures/neboloop/researcher/ROLE.md` | `tests/fixtures/neboloop/researcher/AGENT.md` |
| `tests/fixtures/neboloop/researcher/role.json` | `tests/fixtures/neboloop/researcher/agent.json` |

### Identifier Renames (Rust)

| Current | New |
|---|---|
| `RoleConfig` | `AgentConfig` |
| `RoleDef` | `AgentDef` |
| `RoleTrigger` | `AgentTrigger` |
| `RoleActivity` | `AgentActivity` |
| `RoleInputField` | `AgentInputField` |
| `RoleWorker` | `AgentWorker` |
| `RoleWorkerRegistry` | `AgentWorkerRegistry` |
| `RoleTool` | `AgentTool` |
| `ActiveRole` | `ActiveAgent` |
| `RoleRegistry` | `AgentRegistry` |
| `LoadedRole` | `LoadedAgent` |
| `parse_role` / `parse_role_config` | `parse_agent` / `parse_agent_config` |
| `scan_installed_roles` / `scan_user_roles` | `scan_installed_agents` / `scan_user_agents` |
| `list_roles` / `get_role` / `create_role` | `list_agents` / `get_agent` / `create_agent` |
| `upsert_role_workflow` / `list_role_workflows` | `upsert_agent_workflow` / `list_agent_workflows` |
| `register_role_triggers` / `unregister_role_triggers` | `register_agent_triggers` / `unregister_agent_triggers` |
| `execute_role_workflow_task` | `execute_agent_workflow_task` |
| `cancel_runs_for_role` | `cancel_runs_for_agent` |

### Runtime String Literals

| Current | New |
|---|---|
| `"role:{id}:{binding}"` | `"agent:{id}:{binding}"` |
| `"role-{id}-{binding}"` | `"agent-{id}-{binding}"` |
| `"role:{role_id}"` (workflow_id) | `"agent:{agent_id}"` |
| `role_installed` / `role_activated` (WS events) | `agent_installed` / `agent_activated` |
| `/api/v1/roles` | `/api/v1/agents` |
| `ROLE.md` / `role.json` (filename constants) | `AGENT.md` / `agent.json` |
| `ROLE-XXXX-XXXX-XXXX` (ID prefix) | `AGNT-XXXX-XXXX-XXXX` |
| `"user/roles/"` / `"nebo/roles/"` (dirs) | `"user/agents/"` / `"nebo/agents/"` |

### Execution Order

1. Rename files (filesystem moves)
2. Update `mod` declarations in `lib.rs` files
3. Find-replace Rust identifiers
4. Update HTTP route registration in `server/lib.rs`
5. Update frontend (API calls, stores, component labels)
6. Update docs
7. Update fixtures
8. `cargo build` — surfaces anything missed

---

## 10. What Exists vs. What Needs Building

### Exists Today

| Component | Location |
|---|---|
| `spawn_parallel()` + `FuturesUnordered` | `crates/agent/src/orchestrator.rs` |
| `pending_tasks` with `parent_task_id` | `crates/db/migrations/0022_pending_tasks.sql` |
| `commander_teams` / `commander_edges` schema | `crates/db/migrations/0066_commander.sql` |
| `agents` table with `agent_md` | Migration `0070` (renamed from roles) |
| Session isolation by session key | `crates/agent/src/session.rs` |
| EventBus emit/subscribe | `crates/workflow/src/events.rs` |
| CancellationToken cascade | `crates/agent/src/orchestrator.rs` |
| DAG execution with reactive scheduling | `crates/agent/src/orchestrator.rs` |

### Needs Building

| Component | Capability | Complexity |
|---|---|---|
| Commander dispatch tool → agent identity resolution | Commander | Medium |
| Session key namespacing enforced at dispatch layer | Isolation | Low |
| `session_mode` flag on agents (persistent vs. concurrent) | Concurrent | Low |
| Concurrent instance session lifecycle (create, cleanup) | Concurrent | Medium |
| Memory suppression for ephemeral instances | Concurrent | Low |
| Commander tool callable by primary agent's LLM | Commander | Medium |
| Service registry (named services with config) | Service/Pod | Medium |
| Work queue per service (durable, ordered) | Service/Pod | Medium |
| Instance pool manager (spawn, drain, auto-scale) | Service/Pod | High |
| Back-pressure handling | Service/Pod | Medium |
| Pipeline definition format (YAML/JSON) | Pipelines | Medium |
| Pipeline installer / registry | Pipelines | Medium |
| Stage transition routing (emit → next service queue) | Pipelines | Low* |
| Marketplace pipeline packages | Pipelines | Medium |

*Low because EventBus + EventDispatcher already does this structurally.

### Build Order

**Phase 1 — Foundation (enables Commander + Isolation)**
- Rename ROLE → AGENT throughout
- Session key namespacing enforcement
- `session_mode` flag
- Commander tool wired to agent identity resolution

**Phase 2 — Concurrency**
- Concurrent instance lifecycle
- Memory suppression for ephemeral instances
- Batch fan-out in Commander tool

**Phase 3 — Service/Pod**
- Service manifest in agent config
- Work queue per service
- Instance pool manager
- Auto-scaling

**Phase 4 — Pipelines**
- Pipeline definition format
- Pipeline installer
- Stage transition routing
- Marketplace pipeline packages

---

## 11. Key Files Reference

| File | Relevance |
|---|---|
| `crates/agent/src/orchestrator.rs` | Core spawn/parallel/DAG execution — foundation for all capabilities |
| `crates/agent/src/role_worker.rs` → `agent_worker.rs` | Agent lifecycle, trigger registration |
| `crates/agent/src/session.rs` | Session key management — isolation enforcement goes here |
| `crates/tools/src/role_tool.rs` → `agent_tool.rs` | ActiveAgent, AgentRegistry |
| `crates/tools/src/orchestrator.rs` | SubAgentOrchestrator trait, SpawnRequest/SpawnResult |
| `crates/workflow/src/events.rs` | EventBus, EventDispatcher — pipeline stage routing |
| `crates/db/migrations/0066_commander.sql` | commander_teams, commander_edges schema |
| `crates/db/migrations/0070_rename_roles_to_agents.sql` | DB rename — baseline for agent terminology |
| `crates/db/migrations/0022_pending_tasks.sql` | Task queue — foundation for service work queue |
| `crates/server/src/workflow_manager.rs` | WorkflowManagerImpl — run_inline, spawn tracking |
| `docs/sme/ROLES_SME.md` → `AGENTS_SME.md` | Full agent system reference (update in place) |

---

*Last updated: 2026-03-29*
