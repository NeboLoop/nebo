# Nebo

Your personal AI assistant that runs locally. One agent, always running, with persistent memory.

## The Nebo Paradigm

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           THE NEBO AGENT                                    │
│                                                                             │
│   • Always running (Go process with SQLite persistence)                     │
│   • Has MEMORY that survives restarts                                       │
│   • Spawns SUB-AGENTS for parallel work (up to 5 concurrent)                │
│   • Proactive via scheduled tasks (cron)                                    │
│                                                                             │
│   Channels (how you reach THE agent):                                       │
│     ┌─────────┐  ┌─────────┐  ┌──────────┐  ┌─────────┐  ┌───────┐        │
│     │ Web UI  │  │   CLI   │  │ Telegram │  │ Discord │  │ Slack │        │
│     └────┬────┘  └────┬────┘  └────┬─────┘  └────┬────┘  └───┬───┘        │
│          │            │            │             │           │             │
│          └────────────┴────────────┴─────────────┴───────────┘             │
│                                    │                                        │
│                             ┌──────┴──────┐                                │
│                             │  MAIN AGENT │                                │
│                             │  (agentic   │                                │
│                             │    loop)    │                                │
│                             └──────┬──────┘                                │
│                                    │                                        │
│          ┌─────────────────────────┼─────────────────────────┐             │
│          │                         │                         │             │
│    ┌─────┴─────┐            ┌─────┴─────┐            ┌─────┴─────┐        │
│    │ Sub-Agent │            │ Sub-Agent │            │ Sub-Agent │        │
│    │ (explore) │            │ (research)│            │  (code)   │        │
│    └───────────┘            └───────────┘            └───────────┘        │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

**This is NOT a multi-agent chat system.** Nebo is ONE intelligent agent that:
- Maintains persistent memory across sessions
- Can work on complex tasks by spawning temporary sub-agents
- Connects to you through whatever channel is convenient
- Runs scheduled tasks proactively (even when you're not there)

## Features

- **Persistent Memory** - SQLite-backed conversation history, facts, and preferences
- **Sub-Agent System** - Spawn parallel workers for complex tasks
- **Multi-Provider** - Anthropic, OpenAI, Google Gemini, DeepSeek, Ollama
- **Computer Control** - Browser automation, screenshots, file operations, shell commands
- **Multi-Channel** - Web UI, CLI, Telegram, Discord, Slack
- **Extensible** - YAML skills and compiled plugins
- **Proactive** - Scheduled tasks via cron tool

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/localrivet/nebo/main/install.sh | sh
```

## Quick Start

```bash
# Start Nebo (server + agent + web UI)
nebo

# Open the web UI to add your API key
open http://local.nebo.bot:27895/settings/providers
```

Or use the CLI:

```bash
nebo chat "Hello, what can you do?"
nebo chat --interactive    # REPL mode
```

## How Sub-Agents Work

When THE agent encounters a complex task, it can spawn sub-agents to work in parallel:

```
User: "Analyze this codebase and create a security report"

THE AGENT:
├─► spawn("Explore authentication code", type=explore)
│       └─► Sub-Agent 1: searching, reading files...
│
├─► spawn("Find all API endpoints", type=explore)
│       └─► Sub-Agent 2: grepping, analyzing routes...
│
├─► spawn("Check for hardcoded secrets", type=explore)
│       └─► Sub-Agent 3: scanning for patterns...
│
└─► (waits for all sub-agents, then synthesizes report)
```

Sub-agents are:
- **Temporary** - They complete their task and are cleaned up
- **Focused** - Each has a single objective
- **Parallel** - Unlimited concurrent goroutines within THE agent
- **Internal** - Users don't interact with them directly

> **Note:** Only ONE Nebo instance runs per computer (enforced by lock file). Sub-agents are NOT separate processes—they're parallel workers inside THE agent.

## Configuration

Add your API key in the Web UI: **Settings > Providers**

Or configure via `~/.nebo/models.yaml`:

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

## Built-in Tools

| Tool | Description |
|------|-------------|
| `bash` | Execute shell commands |
| `read` | Read file contents |
| `write` | Create/overwrite files |
| `edit` | Find-and-replace edits |
| `glob` | Find files by pattern |
| `grep` | Search file contents |
| `web` | Fetch URLs |
| `browser` | CDP browser automation |
| `screenshot` | Capture screen/window |
| `vision` | Analyze images with AI |
| `memory` | Persistent fact storage |
| `cron` | Schedule recurring tasks |
| `task` | Spawn sub-agents |
| `agent_status` | Monitor sub-agents |

## Skills System

Skills are YAML files that enhance agent behavior for specific tasks.

```yaml
# ~/.nebo/skills/security-audit.yaml
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
nebo                  Start server + agent + web UI
nebo serve            Server only (for remote agents)
nebo agent            Agent only (connects to server)
nebo chat "prompt"    One-shot chat
nebo chat -i          Interactive REPL
nebo chat --dangerously   No approval prompts (caution!)
```

## Development

```bash
make air              # Backend with hot reload
cd app && pnpm dev    # Frontend dev server
go test ./...         # Run tests
make build            # Build binary
```

## Architecture

```
nebo/
├── agent/                    # The Agent
│   ├── ai/                   # AI providers (Anthropic, OpenAI, etc.)
│   ├── runner/               # Agentic loop
│   ├── orchestrator/         # Sub-agent management
│   ├── tools/                # Built-in tools
│   ├── memory/               # Persistent storage
│   ├── session/              # Conversation history
│   └── skills/               # YAML skill loader
├── internal/                 # Server
│   ├── agenthub/             # Agent WebSocket hub
│   ├── channels/             # Telegram/Discord/Slack
│   └── server/               # HTTP server (chi)
├── app/                      # Web UI (SvelteKit)
└── extensions/               # Bundled skills & plugins
```

## Author

**Alma Tuck**
- Website: [almatuck.com](https://almatuck.com)
- LinkedIn: [linkedin.com/in/almatuck](https://linkedin.com/in/almatuck)
- X: [@almatuck](https://x.com/almatuck)

## License

MIT
