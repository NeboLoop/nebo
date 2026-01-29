# GoBot

An open-source AI agent platform with computer control capabilities. Features a powerful CLI agent, extensible plugin system, and multi-channel integrations.

## Features

- **Autonomous AI Agent** - Agentic loop with tool use, self-correction, and provider failover
- **Computer Control** - Browser automation, screenshots, file operations, shell commands
- **Extensible** - Hot-loadable skills (YAML) and plugins (compiled binaries)
- **Multi-Channel** - Telegram, Discord, Slack integrations
- **Multi-Provider** - Anthropic, OpenAI, Google Gemini, DeepSeek, Ollama
- **Persistent Sessions** - SQLite-backed conversation history with compaction
- **MCP Server** - Expose tools to other AI assistants

## Quick Start

```bash
# Build gobot
make build

# Configure (creates ~/.gobot/)
./bin/gobot config init

# Set your API key
export ANTHROPIC_API_KEY=sk-ant-...

# Start chatting
./bin/gobot chat "Hello, what can you do?"

# Interactive mode
./bin/gobot chat --interactive
```

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│              GoBot CLI Agent                                    │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │  AI Providers          │  Tool Registry                 │   │
│  │  - Anthropic           │  - bash, read, write, edit     │   │
│  │  - OpenAI              │  - glob, grep, web             │   │
│  │  - Google Gemini       │  - browser, screenshot         │   │
│  │  - DeepSeek, Ollama    │  - vision, memory, cron        │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │  Skills (YAML)         │  Plugins (Binaries)            │   │
│  │  - code-review         │  - Custom tools                │   │
│  │  - git-workflow        │  - Channel adapters            │   │
│  │  - security-audit      │  - Hot-loadable                │   │
│  │  - debugging           │                                │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │  SQLite Sessions       │  Config (~/.gobot/)            │   │
│  │  - Conversation history│  - models.yaml (credentials)   │   │
│  │  - Auto-compaction     │  - config.yaml (policies)      │   │
│  └─────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

## Configuration

Configuration is split into two files in `~/.gobot/`:

### `models.yaml` - Provider Credentials & Available Models

```yaml
version: "1.0"
updatedAt: "2026-01-28T00:00:00Z"

# Provider credentials (env vars supported)
credentials:
  anthropic:
    api_key: ${ANTHROPIC_API_KEY}
  openai:
    api_key: ${OPENAI_API_KEY}
  google:
    api_key: ${GOOGLE_API_KEY}
  deepseek:
    api_key: ${DEEPSEEK_API_KEY}
  ollama:
    base_url: http://localhost:11434
  claude-code:                    # CLI provider (wraps claude CLI)
    command: claude
    args: "--print"

# Available models - set active: false to disable
# GoBot autonomously chooses which model to use based on task
providers:
  anthropic:
    - id: claude-sonnet-4-5
      displayName: Claude Sonnet 4.5
      contextWindow: 200000
      active: true
      pricing:
        input: 3.00      # $ per 1M tokens
        output: 15.00
  openai:
    - id: gpt-5.2
      displayName: GPT-5.2
      contextWindow: 400000
      active: true
      pricing:
        input: 1.75
        output: 14.00
        cachedInput: 0.18
  google:
    - id: gemini-2.5-flash
      displayName: Gemini 2.5 Flash
      contextWindow: 1000000
      active: true
      pricing:
        input: 0.30
        output: 2.50
  deepseek:
    - id: deepseek-chat
      displayName: DeepSeek Chat
      contextWindow: 128000
      active: true
      pricing:
        input: 0.28
        output: 0.42
  ollama:
    - id: llama3.3
      displayName: Llama 3.3
      contextWindow: 128000
      active: true
```

### `config.yaml` - Agent Settings & Tool Policies

```yaml
max_context: 50               # Messages before auto-compaction
max_iterations: 100           # Safety limit per agentic run

# Tool approval policy
policy:
  level: allowlist            # deny, allowlist, or full
  ask_mode: on-miss           # off, on-miss, or always
  allowlist:
    - ls
    - pwd
    - cat
    - grep
    - git status
    - git log
    - git diff
```

## Built-in Tools

| Tool | Description | Requires Approval |
|------|-------------|-------------------|
| `bash` | Execute shell commands | Yes (configurable) |
| `read` | Read file contents | No |
| `write` | Create/overwrite files | Yes |
| `edit` | Find-and-replace edits | Yes |
| `glob` | Find files by pattern | No |
| `grep` | Search file contents | No |
| `web` | Fetch URLs | No |
| `browser` | CDP browser automation | Yes |
| `screenshot` | Capture screen/window | No |
| `vision` | Analyze images with AI | No |
| `memory` | Persistent fact storage | No |
| `cron` | Schedule recurring tasks | Yes |
| `task` | Spawn sub-agents for parallel work | No |
| `agent_status` | Check/list/cancel sub-agents | No |

## Sub-Agent System

The `task` tool enables spawning autonomous sub-agents for complex, multi-step tasks:

```
Parent Agent
    │
    ├─► spawn_agent("Research Go testing frameworks", wait=false)
    │       └─► Sub-Agent 1 (exploring, reading docs)
    │
    ├─► spawn_agent("Find authentication bugs", wait=false)
    │       └─► Sub-Agent 2 (searching codebase)
    │
    └─► spawn_agent("Write API documentation", wait=true)
            └─► Sub-Agent 3 (blocking until complete)
```

**Features:**
- **Concurrent execution** - Up to 5 sub-agents running in parallel
- **Specialized agents** - `explore`, `plan`, or `general` agent types
- **Background tasks** - Non-blocking with status checking
- **Automatic cleanup** - Completed agents are cleaned up automatically

**Example LLM usage:**
```json
{"name": "task", "input": {
  "description": "Research testing",
  "prompt": "Find and analyze all test files, report coverage gaps",
  "agent_type": "explore",
  "wait": false
}}
```

---

# Skills System

Skills are **declarative YAML definitions** that enhance agent behavior without code changes. Drop a file and it's immediately active.

## How Skills Work

```
User: "Review my authentication code"
         │
         ▼
┌─────────────────────────────────────┐
│ Skill Matcher                       │
│ Triggers: ["review", "code review"] │
│ Result: MATCH → code-review skill   │
└─────────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────────┐
│ Enhanced System Prompt              │
│ + Skill template                    │
│ + Few-shot examples                 │
│ = More focused, expert response     │
└─────────────────────────────────────┘
```

## Creating Skills

Place YAML files in `~/.gobot/skills/` or `extensions/skills/`:

```yaml
# extensions/skills/security-audit.yaml
name: security-audit
description: Security-focused code analysis
version: "1.0.0"
priority: 20

triggers:
  - security
  - audit
  - vulnerability
  - owasp

tools:
  - read
  - grep
  - glob

template: |
  When performing a security audit:

  1. Check for OWASP Top 10 vulnerabilities:
     - Injection (SQL, command, LDAP)
     - Broken authentication
     - Sensitive data exposure
     - XXE, XSS, insecure deserialization
     - Security misconfiguration
     - Broken access control

  2. Look for:
     - Hardcoded secrets/credentials
     - Unvalidated user input
     - Missing authentication/authorization
     - Insecure cryptography
     - Logging sensitive data

  3. Report format:
     - CRITICAL: Exploitable now
     - HIGH: Serious but needs conditions
     - MEDIUM: Should fix
     - LOW: Best practice improvement

examples:
  - user: "Check this code for security issues"
    assistant: |
      I'll perform a security audit focusing on OWASP Top 10.

      **CRITICAL:**
      - `auth.go:45` - SQL injection via string concatenation
        Fix: Use parameterized queries

      **HIGH:**
      - `config.go:12` - API key hardcoded
        Fix: Use environment variables
```

## Managing Skills

```bash
gobot skills list                           # List all skills
gobot skills show security-audit            # Show skill details
gobot skills test security-audit "audit this code"  # Test matching
```

## Bundled Skills

| Skill | Triggers | Purpose |
|-------|----------|---------|
| `code-review` | review, critique | Structured code reviews |
| `git-workflow` | commit, git, pr | Git operations help |
| `security-audit` | security, audit | OWASP-focused analysis |
| `api-design` | api, endpoint | RESTful API best practices |
| `database-expert` | sql, database | Query optimization |
| `debugging` | debug, error, fix | Systematic debugging |

---

# Plugin System

Plugins are **compiled binaries** that extend GoBot with new tools. They run as separate processes via RPC.

## Plugin Directory

```
~/.gobot/plugins/
├── tools/              # Tool plugins
│   ├── weather         # Binary
│   └── database        # Binary
└── channels/           # Channel plugins
    └── custom-chat     # Binary
```

## Creating a Tool Plugin

```go
// main.go
package main

import (
    "context"
    "encoding/json"
    "github.com/hashicorp/go-plugin"
)

var Handshake = plugin.HandshakeConfig{
    ProtocolVersion:  1,
    MagicCookieKey:   "GOBOT_PLUGIN",
    MagicCookieValue: "gobot-plugin-v1",
}

type MyTool struct{}

func (t *MyTool) Name() string { return "my-tool" }
func (t *MyTool) Description() string { return "Does something useful" }
func (t *MyTool) Schema() json.RawMessage {
    return json.RawMessage(`{"type":"object","properties":{}}`)
}
func (t *MyTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
    return &ToolResult{Content: "Done!", IsError: false}, nil
}
func (t *MyTool) RequiresApproval() bool { return false }

// ... RPC boilerplate (see extensions/tools/example/)
```

Build and install:
```bash
go build -o ~/.gobot/plugins/tools/my-tool
```

---

# CLI Reference

```
gobot [command]

Commands:
  chat          Chat with the AI assistant
    -i, --interactive    Interactive mode
    --dangerously        100% autonomous (no approval prompts)
    -s, --session        Session key (default: "default")
    -p, --provider       Provider to use
    -v, --verbose        Show tool calls

  agent         Connect to SaaS as remote agent
    --org               Organization ID
    --server            Server URL
    --token             JWT token
    --dangerously       100% autonomous (no approval prompts)

  mcp           Start MCP server
    --host              Listen host (default: localhost)
    --port              Listen port (default: 8080)

  config        Configuration management
    init                Create default config

  session       Session management
    list                List sessions
    clear [key]         Clear history

  skills        Skill management
    list                List skills
    show [name]         Show details
    test [name] [input] Test matching

  plugins       Plugin management
    list                List plugins

Global Flags:
  --config      Config file path
  -h, --help    Help
```

---

# Development

```bash
# Build
make build              # Build unified binary (server + agent)
make cli                # Build and install globally

# Test
go test ./...           # Run all tests

# Development
make air                # Backend with hot reload
cd app && pnpm dev      # Frontend dev server
```

## Project Structure

```
gobot/
├── agent/                    # CLI Agent
│   ├── ai/                   # AI providers
│   ├── config/               # Configuration
│   ├── tools/                # Built-in tools
│   ├── runner/               # Agentic loop
│   ├── session/              # SQLite persistence
│   ├── skills/               # Skill loader
│   ├── plugins/              # Plugin loader
│   └── mcp/                  # MCP server
├── cmd/gobot/                # Unified CLI entry point
├── internal/                 # SaaS backend
│   ├── agenthub/             # Agent WebSocket hub
│   ├── channels/             # Telegram/Discord/Slack
│   ├── router/               # Message routing
│   └── ...
├── extensions/               # Bundled extensions
│   ├── skills/               # YAML skills
│   └── plugins/              # Example plugins
└── app/                      # SvelteKit frontend
```

## Tech Stack

| Component | Technology |
|-----------|------------|
| Agent | Go 1.25+, Cobra CLI |
| Backend | go-zero framework |
| Frontend | SvelteKit 2, Svelte 5 |
| Database | SQLite (modernc.org/sqlite) |
| Browser | chromedp (CDP) |
| Plugins | hashicorp/go-plugin |

---

# Agent Mode (SaaS)

Connect the CLI agent to a GoBot server for remote task execution:

```bash
gobot agent --org acme --server https://gobot.example.com --token <jwt>
```

This enables:
- Remote tasks from web dashboard
- Channel integrations (Telegram → Agent → Response)
- Multi-agent coordination
- Centralized history

---

# MCP Server

Expose GoBot's tools to other AI assistants:

```bash
gobot mcp --port 8080
```

Claude Desktop and other MCP clients can then use GoBot's tools.

## Author

**Alma Tuck**
- Website: [almatuck.com](https://almatuck.com)
- X: [@almatuck](https://x.com/almatuck)

## License

MIT
