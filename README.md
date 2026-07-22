# Turn your computer into an AI workforce.

**Nebo is the operating system for AI employees — open source, MIT licensed.**

Hire pre-built employees from the marketplace — bookkeeper, researcher, scheduler — one click each. They show up already knowing the job: workflows, tools, and skills wired together, no setup.

They coordinate as a team, with real handoffs. They work in repeatable, auditable workflows — the thousandth run as dependable as the first. And they operate inside guardrails written into the code, not the prompt, so you can trust them with real access to your systems.

Start on your own machine — your data never leaves your walls. Move to a server when the team needs to grow and never clock out.

```bash
brew install --cask neboloop/tap/nebo
```

Windows and Linux installers in the [latest release](https://github.com/NeboLoop/nebo/releases/latest) — full instructions [below](#install).

**What is Nebo?** Nebo runs a team of AI employees on hardware you control. Each employee is a pre-built role hired from the marketplace in one click. You own the machine, the data, and the workforce.

## Hire, don't build

You've been building agents one at a time. Nebo lets you hire them. Pick a role — bookkeeper, researcher, scheduler — and click once. A pre-built employee arrives with its workflows, tools, and skills already wired together. The shelf is stocked: 110 ready-to-hire roles in the [marketplace](https://neboai.com) today, built from 280 plugins and skills. No configuration, no prompt engineering, no assembly.

<!-- screenshot: marketplace roles page → one-click hire → employee active in the roster. Caption: "Hired in one click. Working in the next." -->

## The work moves between them

A workforce isn't a pile of chatbots. Hand the researcher a job and it delegates — passes its findings to the writer, gets the draft back, sends it on to the scheduler — every handoff landing in the thread where you can read it. Chain workflows so one employee's finished job kicks off the next one's. You manage the team; they manage the work.

<!-- screenshot (LOAD-BEARING): a delegation in the chat thread — researcher's message, then the writer's response with its agent badge. This is shootable today. Caption: "A real handoff, not a metaphor." -->

## The thousandth run is as good as the first

A business doesn't run on improvisation — it runs on process. Nebo work runs as repeatable, auditable workflows that do not drift: the process you approved is the process that executes, run after run, and you can read every step it took.

Some platforms let the AI rewrite its own processes as it goes. We think that's backwards. Self-improving *skills* make an employee sharper at a task; self-improving *processes* governed by AI alone are how a business quietly destabilizes. Business trust is built the way it has always been built — plan, design, implement, audit, measure, and then improve under human guidance. Your processes get better because you decided how, not because they mutated overnight.

<!-- screenshot: workflow run history / audit trail of a completed job. Caption: "Every step recorded. Every run repeatable." -->

## Guardrails in the code, not the prompt

You can't give real system access to something whose safety is a suggestion. Nebo enforces eight layers of defense in code — what an employee can touch, run, and reach is bounded before it ever acts, not politely requested in a prompt. See [SECURITY.md](SECURITY.md) for all eight. That's what makes real access rational instead of reckless.

<!-- screenshot: permission/approval gate — an employee asking, the boundary visible. Caption: "The employee asks. The system enforces." -->

## Install

### macOS

```bash
# Homebrew (recommended)
brew install --cask neboloop/tap/nebo

# Or download the .dmg installer from the latest release
```

### Windows

Download the installer (`Nebo-setup.exe`) from the [latest release](https://github.com/NeboLoop/nebo/releases/latest).

### Linux

```bash
# Debian/Ubuntu (.deb)
# Download from the latest release, then:
sudo dpkg -i nebo_*.deb

# Or build from source
git clone https://github.com/NeboLoop/nebo.git
cd nebo && cargo build --release
```

## Quick Start

```bash
# Desktop mode — native window + system tray (default)
nebo

# Headless mode — opens in your browser
nebo --headless

# CLI chat
nebo chat "What can you do?"
nebo chat -i    # Interactive mode
```

Web UI runs at `http://localhost:27895`. Add your API key in **Settings > Providers**. The UI speaks 25 languages, auto-detected from your system.

## Multi-Provider

Nebo works with the model you prefer:

- **Anthropic** (Claude) — streaming, tool calls, extended thinking
- **OpenAI** (GPT) — streaming, tool calls
- **Google Gemini** — streaming, tool calls
- **Ollama** — local models, no API key needed
- **DeepSeek** — streaming, tool calls via OpenAI-compatible API
- **CLI wrappers** — `claude`, `gemini`, `codex` commands

Configure providers via the Web UI or `models.yaml` in your data directory.

## Built-in Capabilities

| Domain | What it does |
|--------|-------------|
| **File** | Read, write, edit, search files and code |
| **Shell** | Execute commands, manage processes, background tasks |
| **Web** | Fetch pages, search the web, full browser automation |
| **Memory** | Store and recall facts, preferences, project context |
| **Tasks** | Spawn parallel sub-agents, schedule recurring jobs |
| **Communication** | Inter-agent messaging via NeboAI |

Platform-specific capabilities (macOS: accessibility, calendar, contacts; Windows/Linux: desktop automation) are auto-detected.

## Architecture

Nebo is a Rust workspace — one binary, no runtime dependencies beyond SQLite.

### Workspace Crates

| Crate | Purpose |
|-------|---------|
| `types` | Error enum, constants, shared types |
| `config` | Config structs, YAML loading, CLI detection |
| `db` | SQLite store, migrations, connection pool (r2d2) |
| `auth` | JWT auth, keyring integration, credential encryption |
| `ai` | Provider trait + implementations (Anthropic, OpenAI, Gemini, Ollama, CLI) |
| `tools` | Tool registry, policy, domain tools (STRAP pattern), skills loader |
| `agent` | Runner, session, memory, compaction, advisors, search, steering |
| `server` | Axum HTTP server, handlers, WebSocket, middleware |
| `mcp` | MCP bridge, client, AES-256-GCM encryption |
| `napp` | App platform (manifest validation, sandbox, signing) |
| `workflow` | Workflow execution engine |
| `browser` | Chrome/CDP management, snapshot, native host |
| `comm` | Binary wire protocol, loopback transport, ULID |
| `notify` | System notifications |
| `updater` | Background version checker |
| `voice` | Voice input/output |
| `a2ui` | A2UI protocol toolkit (vendored, MIT) |
| `vm` / `vm-daemon` | Sandboxed VM subsystem |
| `render` / `proto` | Rendering + protocol definitions |
| `cli` | CLI entrypoint |

### Key Technology Choices

- **Axum 0.8** — async HTTP framework with tower middleware
- **SQLite** via rusqlite + r2d2 connection pool (WAL mode)
- **Tauri 2** — desktop app with native window + system tray
- **tokio** — async runtime
- **reqwest** — HTTP client with SSE streaming
- **rust-embed** — SPA static assets embedded in binary

## Local Inference

Run entirely offline with local models: install [Ollama](https://ollama.com), configure it as a provider in Settings, and go. No API key, no cloud, no build-time dependencies.

## App Platform

Nebo has a sandboxed app platform. Developers build `.napp` packages that extend Nebo with new tools, channels, and integrations.

- **Sandboxed** — apps run in isolated directories with gRPC over Unix sockets
- **Deny-by-default permissions** — apps only access what their manifest declares
- **Signed** — ED25519 signature verification for every app binary and manifest
- **Compiled-only** — only native binaries accepted (Go, Rust, C, Zig). No interpreted languages.
- **Distributed via NeboAI** — install apps from the marketplace or deploy privately to your loop

See the [Publisher's Guide](docs/publishers-guide/apps.md) for the developer guide.

## Channels

Reach your Nebo from anywhere:

| Channel | How |
|---------|-----|
| **Web UI** | `http://localhost:27895` — the primary interface |
| **CLI** | `nebo chat` — terminal chat mode |
| **Telegram** | Install the Telegram channel app from the marketplace |
| **Discord** | Install the Discord channel app from the marketplace |
| **Slack** | Install the Slack channel app from the marketplace |

Channel apps are distributed through NeboAI. All channels route to the same agent with the same memory and context.

## NeboAI

[NeboAI](https://neboai.com) is the marketplace that stocks the workforce:

- **Hiring** — ready-to-work roles, and the skills, tools, workflows, and apps they're built from — created by the community
- **Cloud integrations** — pre-built connectors like My Cloud (unified Google Workspace access: Gmail, Drive, Sheets, Docs, Contacts)
- **Inter-agent communication** — your Nebo can collaborate with other Nebos in your loop
- **Secure transport** — WebSocket-based binary protocol with JWT authentication

Nebo works fully offline without NeboAI if you have a local LLM (e.g., Ollama). The marketplace is opt-in.

## Security

Eight layers of defense, each enforced in code — not by prompts. See [SECURITY.md](SECURITY.md) for the full architecture and audit trail.

- Hard safeguards block destructive operations unconditionally (sudo, disk formatting, system paths)
- Origin tagging tracks every request source (user, comm, app, skill, system)
- Configurable tool policies with allowlists and approval flows
- Capability permissions gate what each employee is allowed to do
- App sandboxing with process isolation and ED25519 signature verification
- Compiled-only binary policy — no interpreted languages in the app platform
- Network security: JWT auth, CSRF protection, rate limiting, credentials encrypted at rest (AES-256-GCM)
- Process safety: single-instance lock, WebSocket limits, cooperative cancellation

## System Requirements

| Platform | Requirements |
|----------|-------------|
| **macOS** | macOS 11+ (Apple Silicon or Intel) |
| **Windows** | Windows 10+ (64-bit) |
| **Linux** | Ubuntu 22.04+ or equivalent (amd64/arm64) |

## Development

```bash
# Build
make build                           # Release CLI binary
make build-desktop                   # Tauri desktop app (builds frontend first)

# Test
cargo test                           # All workspace tests (900+)

# Run
make dev                             # Desktop dev mode with hot reload

# Frontend only
cd app && pnpm dev                   # SvelteKit dev server (port 5173)
```

## Author

**NeboAI**
- Website: [neboai.com](https://neboai.com)

## License

Nebo is licensed under the [MIT License](LICENSE). Use it, modify it, build on it — freely.

Bundled and vendored third-party components retain their own licenses; see [THIRD-PARTY.md](THIRD-PARTY.md).

**Trademarks.** The MIT license covers the source code only. The *Nebo* and *NeboAI* names, logos, and brand assets (including those under `src-tauri/icons/`) are trademarks and are **not** licensed for use. You may build on the code, but not ship it under the Nebo name or brand.
