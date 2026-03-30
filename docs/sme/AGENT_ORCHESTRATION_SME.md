# Agent Orchestration, Pipelines & Enterprise Architecture — SME Reference

> Strategic and technical design document covering Commander orchestration,
> isolated agent sessions, concurrent agent instances, and the service/pod
> pipeline model. Establishes Nebo as an enterprise-grade intelligent
> automation platform.

**Created:** 2026-03-29  
**Last verified against source:** 2026-03-29 (Claude Code parallel explore)  
**Status:** Design — Phase 1 ready to implement

---

## Table of Contents

1. [Strategic Context](#1-strategic-context)
2. [Ground Truth — Where We Actually Are](#2-ground-truth--where-we-actually-are)
3. [Architecture Overview](#3-architecture-overview)
4. [Capability 1 — Commander: Directed Agent Orchestration](#4-capability-1--commander-directed-agent-orchestration)
5. [Capability 2 — Isolated Agent Sessions](#5-capability-2--isolated-agent-sessions)
6. [Capability 3 — Concurrent Agent Instances](#6-capability-3--concurrent-agent-instances)
7. [Capability 4 — Service/Pod Model & Processing Pipelines](#7-capability-4--servicepod-model--processing-pipelines)
8. [Intelligence at Every Layer](#8-intelligence-at-every-layer)
9. [Enterprise Positioning](#9-enterprise-positioning)
10. [Rename: ROLE → AGENT](#10-rename-role--agent)
11. [Build Order](#11-build-order)
12. [Key Files Reference](#12-key-files-reference)

---

## 1. Strategic Context

Nebo's agent architecture — combined with the service/pod pipeline model — is
not a chat assistant with plugins. It is an **enterprise-grade intelligent
automation platform** where every processing node has reasoning capability.

This makes a large category of SaaS products obsolete. Traditional SaaS
automates fixed processes with dumb functions. Nebo automates adaptive processes
with intelligent agents. The difference is not incremental — it is architectural.

The comparable open-source entrant (OpenClaw) has generated significant community
buzz with 42 documented use cases. Every single one of those use cases is
achievable on Nebo today or with minimal wiring. Nebo's architecture is
structurally deeper — A2A task lifecycle, AgentCard discovery, proper session
isolation, vector memory, marketplace packaging — OpenClaw has none of these.
The gap is not capability; it is catalog and narrative.

---

## 2. Ground Truth — Where We Actually Are

This section reflects the actual codebase state as discovered by parallel
code exploration on 2026-03-29. It supersedes any assumptions in earlier drafts.

### What Is Real and Running

| Infrastructure | Status | Notes |
|---|---|---|
| Orchestrator (spawn, parallel, DAG) | Production | 1102 lines, all 6 trait methods wired, task recovery on startup |
| EventBus + EventDispatcher | Production | Emit tool → EventBus → pattern-matched dispatch → run_inline() |
| Triggers (schedule, heartbeat, event) | Production | 3 types, wired via AgentWorkerRegistry |
| Lanes (8 FIFO queues) | Production | Concurrency-controlled, adaptive semaphore |
| Runner (agentic loop) | Production | 100 max iterations, 80K context, tool chaining, streaming |
| Session isolation by key | Production | `agent:`, `subagent:` prefixes parsed and used |
| Agent registry + worker lifecycle | Production | Activate/deactivate, trigger registration, cleanup |
| Commander graph | Production | 4 tables, 12 queries, 7 REST endpoints, dynamic event edge computation |
| pending_tasks | Production | parent_task_id column exists but FK not populated (uses ID-prefix convention) |
| TaskGraph + Decompose | Built but dormant | DAG structure, Kahn's cycle detection, LLM decomposition — no entry point triggers it |
| A2A types | Production | TaskStatus, AgentCard, TaskArtifact in comm/types.rs — full lifecycle types built |
| Vector embeddings | Production | 0016_vector_embeddings.sql — semantic search infrastructure exists |

### What Does Not Exist Yet

| Component | Vision Says | Reality |
|---|---|---|
| Commander tool | Dispatch to named agents | Commander is visualization only — no execution pathway |
| session_mode | persistent vs concurrent | No field anywhere (DB, config, entity_config) |
| Agent dispatch | Send work TO an active agent | Agents react to triggers; nothing pushes tasks to them |
| Service/Pod model | Work queues, instance pools, auto-scaling | Nothing — single-process, no service registration |
| Pipeline system | Chained services, stage routing | Nothing — no definitions, registry, or routing |
| ~~persona: prefix~~ | ~~Agent-scoped sessions~~ | Consolidated into `agent:` prefix (migration 0071) |
| Concurrent instances | Same agent × N in parallel | No instance lifecycle, no memory suppression |
| Marketplace pipeline packages | Installable pipeline bundles | No format, no installer |

### The Accurate Gap Statement

The codebase has excellent primitives — EventBus, lanes, orchestrator, session
isolation, task persistence, A2A types, vector memory. But the orchestration
layer that connects them for the multi-agent vision doesn't exist yet.

Currently:
- Agents are chatbots + trigger-responders, not workers that accept dispatched jobs
- Sub-agents are anonymous (explore/plan/general), not named specialists  
- Commander is a visualization dashboard, not a control plane
- There is no concept of "this agent processes items from a queue"

The good news: the foundation is genuinely solid. EventBus could route pipeline
stages today. Orchestrator's spawn_parallel could fan out concurrent instances.
pending_tasks could back a work queue. The primitives exist — they need
orchestration glue, not new infrastructure.

---

## 3. Architecture Overview

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

## 4. Capability 1 — Commander: Directed Agent Orchestration

### What It Is

Commander is the deliberate routing layer. A primary agent **decides** to
delegate a specific task to a specific named agent. The caller has intent about
*who* handles the work.

### Current State (from code exploration)

Commander graph infrastructure exists and is production-ready:
- `commander_teams`, `commander_team_members`, `commander_edges`,
  `commander_node_positions` tables all present
- 7 REST endpoints wired
- Dynamic event edge computation running
- **Gap:** No execution pathway. Commander is visualization only.
  Nothing reads commander_edges to make routing decisions.

### What Needs Building

**1. Commander dispatch tool** — a tool callable by the primary agent's LLM:
```
dispatch_to_agent(agent_name, task, wait=false) → task_id
```

This tool:
- Reads `commander_edges` to verify the calling agent has a relationship
  with the target
- Resolves target agent from `agents` table by name/id
- Pulls `agent_md` as the system prompt (not generic explore/plan/general)
- Loads agent's skills, MCPs, tool permissions from `agent_workflows` / `entity_config`
- Derives session key: `agent:{agent_id}` (isolated, persistent)
- Dispatches via existing `Orchestrator.spawn()` with `wait: false`
- Records `pending_task` with `parent_task_id` linking back to caller

**2. Result callback** — when specialist completes, primary agent receives
result. Uses existing `pending_tasks.output` + WebSocket notification.
The `parent_task_id` FK convention already exists — just needs population.

**3. Commander as control plane** — the Commander graph UI becomes the
visual representation of live agent relationships AND the configuration
surface for who can dispatch to whom.

### Session Key for Dispatched Agents

Dispatched specialist agents use: `agent:{agent_id}`

This key is already parsed in keyparser. The dispatch tool just needs to
generate it correctly when building the SpawnRequest.

---

## 5. Capability 2 — Isolated Agent Sessions

### Current State

Session isolation by key prefix is production-ready. The keyparser handles
`agent:` and `subagent:` prefixes. Memory extraction runs per session key.
The isolation mechanism works — what's missing is enforcement at the
dispatch layer.

### What Needs Building

**session_mode flag** — new column on `agents` table (or `entity_config`):
- `persistent` (default) — one session per agent, accumulates memory over time
- `concurrent` — each job gets an isolated ephemeral instance session

**Dispatch enforcement** — Commander dispatch tool always constructs session
key from the target agent's own ID. Never passes the calling agent's session
key through. This is a rule in the dispatch tool, not a configuration option.

**agent: prefix** — now the canonical prefix for agent-scoped sessions
(consolidated from the former `persona:` prefix via migration 0071).
Agent chat sessions use `agent:{agent_id}:{channel}` keys.

### Key: No New Infrastructure Needed

Session isolation already works. This is entirely about ensuring the dispatch
layer generates the right session key and that session_mode controls whether
that key is stable (persistent) or instance-scoped (concurrent).

---

## 6. Capability 3 — Concurrent Agent Instances

### What It Is

Multiple instances of the same agent type running in parallel, each with
100% isolated context. The Document Processor agent runs as 12 simultaneous
instances, each processing a different document with no cross-contamination.

### Current State

`spawn_parallel()` with `FuturesUnordered` exists and works. `pending_tasks`
with `parent_task_id` tracks batches. CancellationToken cascade works.
The parallel execution infrastructure is production-ready.

**Gap:** No instance lifecycle. No memory suppression for ephemeral instances.
No session_mode to trigger concurrent behavior. No automatic fan-out.

### What Needs Building

**Instance session keys** — when `session_mode = concurrent`, each dispatch
generates a ULID-based instance ID:
`agent:{agent_id}:instance:{ulid}`

Each instance gets its own message history, zero cross-contamination.

**Memory suppression** — concurrent instances skip memory extraction on
completion. They are ephemeral workers, not persistent agents. Add
`skip_memory_extract: true` to SpawnRequest when spawning concurrent instances
(this field already exists on RunRequest).

**Automatic fan-out** — Commander dispatch tool detects `session_mode = concurrent`
and automatically calls `spawn_parallel()` when given a list of inputs,
rather than requiring the caller to request parallelism explicitly.

**Instance cleanup** — configurable: discard session after completion (default)
or retain for audit. Simple scheduled cleanup of `agent:*:instance:*` sessions
older than N hours.

---

## 7. Capability 4 — Service/Pod Model & Processing Pipelines

### What It Is

**Service model:** Load-balanced intake. Work arrives at a named service
endpoint and is claimed by the next available agent instance. Analogous
to Kubernetes Service → Pod. The caller doesn't know which instance handles it.

**Pipelines:** Services chained together via EventBus. Output of one service
becomes input of the next. Every stage is an intelligent agent.

### Current State

- EventBus + EventDispatcher: production-ready, emit/subscribe working
- Lanes (8 FIFO queues): production-ready, could back a service work queue
- pending_tasks: could back a durable work queue per service
- **Nothing exists** for service registration, instance pools, auto-scaling,
  pipeline definitions, or pipeline routing

### What Needs Building

**Phase 3 — Service/Pod:**

Service manifest in agent config:
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

Work queue per service — extend `pending_tasks` with a `service_id` column.
Items enter the queue, instances claim and process them.

Instance pool manager — spawn instances when queue depth exceeds threshold,
drain idle instances when queue is empty. Builds on existing
`AgentWorkerRegistry` lifecycle.

Back-pressure — configurable behavior when queue is full: drop, block, or
reject with error.

**Phase 4 — Pipelines:**

Pipeline definition format (YAML alongside agent definitions):
```yaml
pipeline: email-management
stages:
  - service: email-triage
    on_emit:
      email.urgent: response-draft
      email.invoice: document-processing
  - service: response-draft
    next: send-queue
```

Stage transitions use the existing EventBus — this is structurally already
supported. Pipeline definitions are configuration on top of existing
emit/subscribe infrastructure.

Pipeline installer + registry — installable from marketplace like agents.

---

## 8. Intelligence at Every Layer

This is the fundamental differentiator from traditional pipeline tools.

In conventional pipelines, stages are dumb functions — they transform data
according to fixed rules. Every edge case must be anticipated at design time.

In Nebo pipelines, every stage is an agent with reasoning, tool access,
memory, adaptive routing, and intelligent failure handling.

The compound effect: a 5-stage pipeline where each agent reasons around
problems produces dramatically better outcomes than the same pipeline with
dumb functions. The system gets smarter the more it runs — persistent agents
accumulate memory, pipelines accumulate calibration.

---

## 9. Enterprise Positioning

### The Comparable Systems

Nebo's pipeline architecture occupies the same category as MuleSoft (enterprise
integration), Temporal (durable workflow execution), and UiPath (RPA) — but
with a critical difference: those systems move **data** between **dumb workers**.
Nebo moves **context** between **intelligent agents**.

### The SaaS Displacement Thesis

| Category | Traditional SaaS | Nebo Replacement |
|---|---|---|
| Email management | Front, Superhuman, Help Scout | Email Triage + Response Pipeline |
| Document processing | Docsumo, Rossum, Instabase | Document Processing Agent Service |
| Contract review | Ironclad, Lexion, Kira | Contract Review Pipeline |
| Lead enrichment | Clearbit, ZoomInfo, Clay | Lead Enrichment Agent Service |
| Content workflows | Contentful, Gather Content | Content Creation Pipeline |
| Ad production | Pencil, AdCreative.ai | Ad Creation Pipeline |
| Invoice processing | Hypatos, Ocrolus | Finance Document Pipeline |

### The Marketplace as Enterprise App Store

Publishers ship complete pipeline packages — "Email Management Suite",
"Contract Review Pipeline", "Lead Enrichment System". Enterprises install,
configure with natural language, and have intelligent automation running
in minutes rather than months.

---

## 10. Rename: ROLE → AGENT

Migration `0070` handled the DB layer. The following surfaces remain.

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

### Key Identifier Renames (Rust)

| Current | New |
|---|---|
| `RoleConfig` / `RoleDef` / `RoleTrigger` | `AgentConfig` / `AgentDef` / `AgentTrigger` |
| `RoleWorker` / `RoleWorkerRegistry` | `AgentWorker` / `AgentWorkerRegistry` |
| `RoleTool` / `ActiveRole` / `RoleRegistry` | `AgentTool` / `ActiveAgent` / `AgentRegistry` |
| `LoadedRole` / `RoleSource` | `LoadedAgent` / `AgentSource` |
| `parse_role` / `parse_role_config` | `parse_agent` / `parse_agent_config` |
| `scan_installed_roles` / `scan_user_roles` | `scan_installed_agents` / `scan_user_agents` |
| `list_roles` / `get_role` / `create_role` | `list_agents` / `get_agent` / `create_agent` |
| `execute_role_workflow_task` | `execute_agent_workflow_task` |
| `cancel_runs_for_role` | `cancel_runs_for_agent` |

### Runtime String Literals

| Current | New |
|---|---|
| `"role:{id}:{binding}"` / `"role-{id}-{binding}"` | `"agent:{id}:{binding}"` / `"agent-{id}-{binding}"` |
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

## 11. Build Order

### Phase 1 — Commander + Isolation (enables the core multi-agent vision)

**What gets built:**
- ROLE → AGENT rename throughout (hygiene, unblocks everything)
- `session_mode` flag on agents table (`persistent` | `concurrent`)
- Commander dispatch tool — reads `commander_edges`, resolves agent identity,
  constructs correct session key, dispatches via existing `Orchestrator.spawn()`
- Populate `pending_tasks.parent_task_id` FK properly on dispatch
- Session key enforcement in dispatch layer (not convention)
- ~~Wire `persona:` prefix~~ Done — consolidated into `agent:` prefix (0071)
- Activate dormant `TaskGraph` / `Decompose` — add entry point so primary
  agent can trigger DAG decomposition

**What this unlocks:**
- Primary agent can deliberately dispatch to named specialists
- PI Agent and Travel Agent have completely isolated memory
- Multi-agent team use case (like OpenClaw's Discord team pattern) works natively

### Phase 2 — Concurrency

**What gets built:**
- Instance lifecycle for `concurrent` session_mode agents
- ULID-based instance session key generation
- Memory suppression for ephemeral instances
- Automatic fan-out in Commander dispatch tool when input is a list

**What this unlocks:**
- Document processing at scale
- Any batch job that benefits from parallel isolated execution

### Phase 3 — Service/Pod

**What gets built:**
- Service manifest in agent config
- Work queue per service (extend `pending_tasks` with `service_id`)
- Instance pool manager (builds on `AgentWorkerRegistry`)
- Back-pressure handling

**What this unlocks:**
- High-volume throughput workloads
- Decoupled producer/consumer patterns
- Auto-scaling under load

### Phase 4 — Pipelines

**What gets built:**
- Pipeline definition format (YAML)
- Pipeline installer / registry
- Stage transition routing (emit → next service queue)
- Marketplace pipeline packages

**What this unlocks:**
- Full end-to-end intelligent automation pipelines
- Installable vertical solutions (Email Suite, Contract Review, etc.)
- The enterprise catalog

---

## 12. Key Files Reference

| File | Relevance |
|---|---|
| `crates/agent/src/orchestrator.rs` | Core spawn/parallel/DAG — foundation for all capabilities. 1102 lines, all production. |
| `crates/agent/src/role_worker.rs` → `agent_worker.rs` | Agent lifecycle, trigger registration |
| `crates/agent/src/session.rs` | Session key management — isolation enforcement goes here |
| `crates/agent/src/task_graph.rs` | DAG structure, Kahn's cycle detection — dormant, needs entry point |
| `crates/agent/src/decompose.rs` | LLM task decomposition — dormant, needs entry point |
| `crates/tools/src/role_tool.rs` → `agent_tool.rs` | ActiveAgent, AgentRegistry |
| `crates/tools/src/orchestrator.rs` | SubAgentOrchestrator trait, SpawnRequest/SpawnResult |
| `crates/workflow/src/events.rs` | EventBus, EventDispatcher — pipeline stage routing, production-ready |
| `crates/comm/src/types.rs` | A2A types: TaskStatus, AgentCard, TaskArtifact — production-ready |
| `crates/db/migrations/0066_commander.sql` | commander_teams, commander_edges schema |
| `crates/db/migrations/0070_rename_roles_to_agents.sql` | DB rename baseline |
| `crates/db/migrations/0022_pending_tasks.sql` | Task queue — foundation for service work queue |
| `crates/db/migrations/0016_vector_embeddings.sql` | Semantic memory — production-ready |
| `crates/server/src/workflow_manager.rs` | WorkflowManagerImpl — run_inline, spawn tracking |
| `crates/agent/src/keyparser.rs` | Session key parsing — `agent:`, `subagent:` prefixes |
| `docs/sme/ROLES_SME.md` → `AGENTS_SME.md` | Full agent system reference (update in place) |

---

*Last updated: 2026-03-29 — reflects actual codebase state from parallel code exploration*
