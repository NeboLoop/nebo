# Nebo

Your personal AI assistant that runs locally. One primary agent with a lane-based concurrency system and persistent memory.

## The Nebo Paradigm

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                            THE NEBO AGENT                                    │
│                                                                              │
│   • One primary WebSocket connection (enforced by hub)                       │
│   • Lane-based concurrency for different work types                          │
│   • Spawns SUB-AGENTS as goroutines for parallel work                        │
│   • SQLite persistence survives restarts (crash recovery)                    │
│   • Proactive via heartbeat lane + scheduled events                          │
│   • Inter-agent communication via comm lane + plugins                        │
│                                                                              │
│   Channels (how you reach THE agent):                                        │
│     ┌─────────┐  ┌─────────┐  ┌──────────┐  ┌─────────┐  ┌───────┐         │
│     │ Web UI  │  │   CLI   │  │ Telegram │  │ Discord │  │ Slack │         │
│     └────┬────┘  └────┬────┘  └────┬─────┘  └────┬────┘  └───┬───┘         │
│          │            │            │             │           │              │
│          └────────────┴────────────┴─────────────┴───────────┘              │
│                                    │                                         │
│                        ┌───────────┴───────────┐                            │
│                        │      LANE SYSTEM      │                            │
│                        │  (supervisor pattern) │                            │
│                        └───────────┬───────────┘                            │
│                                    │                                         │
│    ┌────────┬────────┬─────────────┼──────────┬───────────┬──────────┐      │
│    │        │        │             │          │           │          │      │
│  ┌─┴──┐ ┌──┴───┐ ┌──┴────┐ ┌─────┴────┐ ┌───┴──┐ ┌─────┴────┐    │      │
│  │main│ │events│ │subagnt│ │  nested  │ │ hb   │ │   comm   │    │      │
│  │ (1)│ │  (2) │ │       │ │   (3)   │ │ (1)  │ │   (5)   │    │      │
│  └─┬──┘ └──┬───┘ └──┬────┘ └─────┬────┘ └───┬──┘ └─────┬────┘    │      │
│    │        │        │            │          │           │          │      │
│    ▼        ▼        ▼            ▼          ▼           ▼          │      │
│  User    Scheduled  Sub-Agent   Tool      Proactive  Inter-agent   │      │
│  chat    tasks      goroutines  recursion heartbeat  messages      │      │
│                                                                              │
└──────────────────────────────────────────────────────────────────────────────┘
```

**Key Architectural Concepts:**

| Concept | What It Means |
|---------|---------------|
| **One Agent** | Single WebSocket connection to hub. If reconnected, old connection is dropped. |
| **Lane System** | Work queues for different types of work. Main lane serializes user chat (1 at a time). |
| **Sub-Agents** | Goroutines (not separate processes) spawned for parallel work. Each gets its own session. |
| **Crash Recovery** | Pending sub-agent tasks persist to SQLite, recovered on restart. |

**This is NOT a multi-agent chat system.** Users interact with ONE agent that:
- Serializes your conversations (one at a time via main lane)
- Spawns temporary sub-agents for parallel work
- Connects through any channel you prefer
- Runs scheduled tasks concurrently (events lane)

## Features

- **Persistent Memory** - SQLite-backed conversation history, facts, and preferences
- **Sub-Agent System** - Spawn parallel workers for complex tasks
- **Multi-Provider** - Anthropic, OpenAI, Google Gemini, DeepSeek, Ollama
- **Computer Control** - Browser automation, screenshots, file operations, shell commands
- **Multi-Channel** - Web UI, CLI, Telegram, Discord, Slack
- **Inter-Agent Comm** - Plugin-based communication between agents (loopback, MQTT, NATS)
- **Extensible** - YAML skills and compiled plugins
- **Proactive** - Scheduled tasks and heartbeat-driven actions

## Install

```bash
# macOS (desktop app with native window + system tray)
brew install --cask nebolabs/tap/nebo

# macOS/Linux (CLI binary)
brew install nebolabs/tap/nebo

# Or build from source
git clone https://github.com/nebolabs/nebo.git
cd nebo && make build
```

## Quick Start

```bash
# Desktop mode — native window + system tray (default)
nebo

# Headless mode — browser-only, no native window (current behavior)
nebo --headless

# Open the web UI to add your API key
open http://local.nebo.bot:27895/settings/providers
```

Or use the CLI:

```bash
nebo chat "Hello, what can you do?"
nebo chat --interactive    # REPL mode
```

## How Lanes and Sub-Agents Work

**Lanes** are work queues that organize different types of work:

| Lane | Concurrency | Purpose |
|------|-------------|---------|
| `main` | 1 | User conversations (serialized, one at a time) |
| `events` | 2 | Scheduled/triggered tasks |
| `subagent` | unlimited | Sub-agent goroutines |
| `nested` | 3 | Tool recursion/callbacks |
| `heartbeat` | 1 | Proactive heartbeat ticks |
| `comm` | 5 | Inter-agent communication messages |

**Sub-agents** are spawned for parallel work:

```
User: "Analyze this codebase and create a security report"

THE AGENT (main lane, serialized):
├─► spawn("Explore authentication code")
│       └─► Sub-Agent goroutine (subagent lane)
│           └─► Gets own session, runs own agentic loop
│
├─► spawn("Find all API endpoints")
│       └─► Sub-Agent goroutine (subagent lane)
│
├─► spawn("Check for hardcoded secrets")
│       └─► Sub-Agent goroutine (subagent lane)
│
└─► (waits for all sub-agents, synthesizes report)
```

**Sub-agent characteristics:**
- **Goroutines** - NOT separate processes or WebSocket connections
- **Own Session** - Each gets a session key like `subagent-{uuid}`
- **Persisted** - Tasks saved to SQLite for crash recovery
- **Temporary** - Cleaned up after completion

> **Note:** Only ONE Nebo instance runs per computer (lock file enforced). Sub-agents are parallel workers inside THE agent's process.

## Configuration

Add your API key in the Web UI: **Settings > Providers**

Or configure via `models.yaml` in your Nebo data directory:

```yaml
version: "1.0"

defaults:
  primary: anthropic/claude-sonnet-4-5-20250929
  fallbacks:
    - anthropic/claude-haiku-4-5-20250929

providers:
  anthropic:
    - id: claude-sonnet-4-5-20250929
      displayName: Claude Sonnet 4.5
      contextWindow: 1000000
      active: true
  openai:
    - id: gpt-5.2
      displayName: GPT-5.2
      contextWindow: 400000
      active: true
```

## Built-in Tools (STRAP Pattern)

Tools use the **STRAP (Single Tool Resource Action Pattern)** — consolidating many tools into domain tools for reduced LLM context overhead.

| Domain | Tool | Resources / Actions |
|--------|------|---------------------|
| File | `file` | read, write, edit, glob, grep |
| Shell | `shell` | bash exec/bg/kill, process list/kill, session management |
| Web | `web` | fetch, search, browser automation (navigate, click, type, screenshot) |
| Agent | `agent` | task (spawn/status/cancel), cron (create/list/delete), memory (store/recall), message (send/list), session (list/history/clear), comm (send/subscribe/status) |

**Standalone tools:** `screenshot`, `vision` (image analysis), platform-specific capabilities (macOS: accessibility, calendar, contacts, etc.)

## Skills System

Skills are YAML files that enhance agent behavior for specific tasks.

```yaml
# skills/security-audit.yaml (in Nebo data directory)
name: security-audit
triggers: [security, audit, vulnerability]
template: |
  When performing a security audit:
  1. Check for OWASP Top 10 vulnerabilities
  2. Look for hardcoded secrets
  3. Review authentication flows
  ...
```

Bundled skills: `code-review`, `git-workflow`, `security-audit`, `api-design`, `database-expert`, `debugging`

## Advisors System

Advisors are internal "voices" that deliberate on tasks before the agent decides. They do NOT speak to users, commit memory, or persist independently — they only inform the agent's decisions.

```
User: "Should we rewrite the auth system?"
                    │
            ┌───────┴───────┐
            │  THE AGENT    │
            │  (main lane)  │
            └───────┬───────┘
                    │ advisors(task: "Evaluate auth rewrite")
                    │
        ┌───────────┼───────────┬───────────┐
        ▼           ▼           ▼           ▼
   ┌─────────┐ ┌──────────┐ ┌─────────┐ ┌─────────┐
   │ Skeptic │ │Pragmatist│ │Historian│ │Creative │
   │(critic) │ │(builder) │ │(context)│ │(explorer│
   └────┬────┘ └────┬─────┘ └────┬────┘ └────┬────┘
        │           │            │            │
        └───────────┴────────────┴────────────┘
                    │
            Agent synthesizes and decides
```

**Key properties:**
- Run concurrently (up to 5, with 30s timeout)
- Each advisor sees the same context but not other advisors' responses
- The agent is the decision-maker — advisors only provide counsel
- Hot-reload: edit files and changes take effect immediately

### Defining Advisors

Advisors are Markdown files with YAML frontmatter, stored in the `advisors/` directory:

```
advisors/
├── skeptic/
│   └── ADVISOR.md
├── pragmatist/
│   └── ADVISOR.md
├── historian/
│   └── ADVISOR.md
└── creative/
    └── ADVISOR.md
```

Each `ADVISOR.md`:

```markdown
---
name: skeptic
role: critic
description: Challenges assumptions and identifies weaknesses
priority: 10
enabled: true
---

# The Skeptic

You are the Skeptic. Your role is to challenge ideas and find flaws.

## Your Approach
- Question every assumption
- Look for edge cases and failure modes
- Challenge optimistic estimates
```

| Field | Required | Purpose |
|-------|----------|---------|
| `name` | Yes | Unique identifier |
| `role` | No | Category (critic, builder, historian, etc.) |
| `description` | Yes | What this advisor does |
| `priority` | No | Execution order (higher = first, default: 0) |
| `enabled` | No | Disable without deleting (default: true) |
| `memory_access` | No | Enable persistent memory recall for this advisor (default: false) |

The markdown body after the frontmatter is the advisor's persona — the system prompt that shapes their voice.

### Using Advisors

The agent decides when to consult advisors via the `advisors` tool:

```
advisors(task: "Should we migrate from REST to GraphQL?")
advisors(task: "Evaluate caching strategy", advisors: ["skeptic", "pragmatist"])
```

**Use advisors for:** Significant decisions, multiple valid approaches, when uncertain.
**Skip advisors for:** Simple, routine, or time-sensitive tasks.

### Configuration

```yaml
# config.yaml (in Nebo data directory)
advisors:
  enabled: true           # Disable all advisors globally
  max_advisors: 5         # Max concurrent advisors per deliberation
  timeout_seconds: 30     # Per-deliberation timeout
```

### Built-in Advisors

| Advisor | Role | Memory | Purpose |
|---------|------|--------|---------|
| **Skeptic** | critic | No | Challenges assumptions, identifies weaknesses |
| **Pragmatist** | builder | No | Finds simplest viable action, cuts complexity |
| **Historian** | context | Yes | Brings context from past decisions and patterns |
| **Creative** | explorer | No | Explores novel or unconventional approaches |
| **Optimist** | advocate | No | Focuses on possibilities and upside potential |

The Historian has `memory_access: true`, so it receives relevant memories from Nebo's persistent memory (hybrid vector + FTS search) before deliberating. Any advisor can opt into memory access by adding this flag to their frontmatter.

## Channel Integrations

Connect Nebo to messaging platforms:

| Channel | Setup |
|---------|-------|
| **Telegram** | Create bot via @BotFather, add token in Settings |
| **Discord** | Create application, add bot token in Settings |
| **Slack** | Create app with Socket Mode, add tokens in Settings |

Messages from these channels are routed to THE agent, and responses are sent back.

## CLI Reference

```
nebo                  Desktop mode (native window + system tray + agent)
nebo --headless       Headless mode (HTTP server + agent, opens browser)
nebo serve            Server only (for remote agents)
nebo agent            Agent only (connects to server)
nebo chat "prompt"    One-shot chat
nebo chat -i          Interactive REPL
nebo chat --dangerously   No approval prompts (caution!)
```

## Development

```bash
make air              # Backend with hot reload (headless mode)
cd app && pnpm dev    # Frontend dev server
go test ./...         # Run tests
make build            # Build binary (headless + desktop)
make desktop          # Build desktop app (includes frontend build)
make package          # Package as installer (.dmg/.msi/.deb)
```

## Architecture

```
nebo/
├── internal/
│   ├── agent/                # The Agent Core
│   │   ├── ai/               # AI providers (Anthropic, OpenAI, Gemini, Ollama)
│   │   ├── comm/             # Inter-agent communication (CommPlugin, handler, manager)
│   │   ├── runner/           # Agentic loop (model selection, tool execution)
│   │   ├── orchestrator/     # Sub-agent spawning + crash recovery
│   │   ├── tools/            # STRAP domain tools (file, shell, web, agent)
│   │   ├── session/          # Conversation persistence + compaction
│   │   ├── embeddings/       # Vector + FTS hybrid search for memories
│   │   ├── skills/           # YAML skill loader (hot-reload)
│   │   ├── plugins/          # hashicorp/go-plugin for extensions
│   │   └── recovery/         # Pending task persistence for crash recovery
│   ├── agenthub/             # WebSocket hub + lane system
│   │   ├── hub.go            # Connection management, message routing
│   │   └── lane.go           # Concurrency queues (main/events/subagent/nested/heartbeat/comm)
│   ├── channels/             # Telegram, Discord, Slack integrations
│   └── server/               # HTTP server (chi router)
├── cmd/nebo/                 # CLI commands (agent, chat, serve, desktop, etc.)
│   ├── desktop.go            # Wails v3 native window + system tray
│   ├── root.go               # Headless mode (RunAll)
│   └── vars.go               # --headless flag routing
├── app/                      # Web UI (SvelteKit 2 + Svelte 5)
├── assets/icons/             # App icons for all platforms
└── extensions/               # Bundled skills & plugins
```

### Component Responsibilities

| Component | Role |
|-----------|------|
| **Hub** (`agenthub/hub.go`) | WebSocket connections, agent registry, message routing |
| **Lanes** (`agenthub/lane.go`) | Work queues with concurrency limits per lane type |
| **Runner** (`agent/runner/`) | Agentic loop: model selection, tool execution, streaming |
| **Orchestrator** (`agent/orchestrator/`) | Sub-agent spawning, task persistence, crash recovery |
| **Session** (`agent/session/`) | Conversation history, context compaction |

### Data Flow

```
Channel (Web/CLI/Telegram/Discord/Slack)
    ↓
Hub receives WebSocket message
    ↓
Routes to Agent via channel
    ↓
Agent command enqueues to Lane
    ↓
Lane worker (respecting concurrency) calls Runner.Run()
    ↓
Runner executes agentic loop (stream events back)
    ↓
Hub broadcasts to connected clients
```

## Author

**Alma Tuck**
- Website: [almatuck.com](https://almatuck.com)
- LinkedIn: [linkedin.com/in/almatuck](https://linkedin.com/in/almatuck)
- X: [@almatuck](https://x.com/almatuck)

## License

MIT
