# Turn your computer into an AI workforce.

**Nebo is the operating system for AI employees — open source, Apache 2.0 licensed.**

Hire pre-built employees from the marketplace — bookkeeper, researcher, scheduler — one click each. They show up already knowing the job: workflows, tools, and skills wired together, no setup. Already running OpenClaw, Hermes, Claude Code, or Codex? Connect them with Nebo Link and hire them onto the same team.

They coordinate as a team, with real handoffs. They work in repeatable, auditable workflows — the thousandth run as dependable as the first. And they operate inside guardrails written into the code, not the prompt, so you can trust them with real access to your systems.

Run it on your own computer, on your own server, or in Nebo Cloud. Your employees, their memory, and your files live where the bot runs, and you can move the team as it grows.

```bash
brew install --cask neboloop/tap/nebo
```

Windows and Linux installers in the [latest release](https://github.com/NeboLoop/nebo/releases/latest) — full instructions [below](#install).

**What is Nebo?** Nebo runs a team of AI employees on your computer, your server, or in Nebo Cloud. Each employee is a pre-built role hired from the marketplace in one click, or an agent you already run, connected with Nebo Link. You own the data and the workforce.

## Hire, don't build

You've been building agents one at a time. Nebo lets you hire them. Pick a role — bookkeeper, researcher, scheduler — and click once. A pre-built employee arrives with its workflows, tools, and skills already wired together. The [marketplace](https://neboai.com) is stocked with ready-to-hire roles, built from plugins and skills. No configuration, no prompt engineering, no assembly.

<!-- screenshot: marketplace roles page → one-click hire → employee active in the roster. Caption: "Hired in one click. Working in the next." -->

## Hire the agents you already run

Your OpenClaw, Hermes, and coding agents (Claude Code, Codex, Gemini CLI, OpenCode, or any agent that speaks the [Agent Client Protocol](https://agentclientprotocol.com)) can join your Nebo workforce without moving. [Nebo Link](https://github.com/NeboLoop/nebo-link) connects the computer they run on to your NeboAI account. It uses outbound connections only, so you don't open ports or set up a VPN.

```bash
nebo-link ABCD-1234                          # pair this computer with the code from the NeboAI app
nebo-link add claude-code --dir ~/code/site  # add a coding agent, working in that folder
nebo-link add codex --dir ~/code/api
```

A linked computer is one bot, and each agent on it is its own employee. In Nebo, open **Hire an employee → Hire from another app** and pick one:

- **It keeps working where it runs.** Nebo drives the agent through the link. The agent keeps its own persona and transcript, and a Claude Code or Codex agent uses its own sign-in on that computer. NeboAI never sees those credentials.
- **It follows your permission setting.** On Full Access it just works. Otherwise, when it needs your OK, the request comes to you on the desktop, on your phone, or in the agent's own UI.
- **It's on the roster like any other hire.** You can rename it like any other employee. Its persona stays on the linked computer, and it shows as offline when that computer is.

Linked agents are also reachable from the NeboAI mobile app. Nebo Link is the reference implementation of [Open Agent Link](https://openagent.link), an open, end-to-end encrypted protocol for reaching agents on any computer.

### Or bring your setup over

To move a Hermes or OpenClaw install into Nebo instead of linking it, Nebo detects it during onboarding or in **Settings → Import**. It shows you everything it found before changing anything. It then imports your employees, skills, MCP servers, memory, conversation history, and provider keys, and it never writes to the original install. Re-running an import won't duplicate anything.

## The work moves between them

A workforce isn't a pile of chatbots. Hand the researcher a job and it delegates — passes its findings to the writer, gets the draft back, sends it on to the scheduler — every handoff landing in the thread where you can read it. Chain workflows so one employee's finished job kicks off the next one's. You manage the team; they manage the work.

<!-- screenshot (LOAD-BEARING): a delegation in the chat thread — researcher's message, then the writer's response with its employee badge. This is shootable today. Caption: "A real handoff, not a metaphor." -->

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

Both amd64 and arm64 builds ship with every release.

### Raspberry Pi (and other ARM64 boards)

Works on 64-bit Raspberry Pi OS (trixie or newer) and any ARM64 distro with
glibc 2.39+ (Ubuntu 24.04+, Debian 13+). Install the arm64 `.deb` as above, or
run the standalone headless binary:

```bash
sudo apt install -y libwayland-client0 libopenblas0
curl -fL -o nebo https://github.com/NeboLoop/nebo/releases/latest/download/nebo-linux-arm64-headless
chmod +x nebo && ./nebo --headless
# → http://localhost:27895 (needs glibc 2.39+, e.g. Ubuntu 24.04 / Pi OS trixie)
```

### Android (Termux)

Runs in [Termux](https://termux.dev) via a proot Ubuntu userland — no root
required. In Termux:

```bash
curl -fsSL https://raw.githubusercontent.com/NeboLoop/nebo/main/scripts/install-android.sh | bash
nebo   # then open http://localhost:27895 in Chrome on the phone
```

### Chromebook

ChromeOS Linux mode (Crostini) ships **Debian 12**, whose glibc (2.36) is older
than these builds require — Nebo will not start there without an upgrade. Two
options that do work:

```bash
# 1. Upgrade the Linux container to Debian 13 (trixie), then install as above
sudo sed -i 's/bookworm/trixie/g' /etc/apt/sources.list /etc/apt/sources.list.d/*.list
sudo apt update && sudo apt full-upgrade -y && sudo reboot

# 2. Or replace the container with Ubuntu 24.04 from ChromeOS settings
```

Both amd64 and arm64 Chromebooks are supported once the container is current.
Simplest of all: skip installing and manage a cloud Nebo from the browser at
[neboai.com](https://neboai.com) — no Linux container needed.

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

Web UI runs at `http://localhost:27895`. Sign in and Nebo routes to a curated provider for you, no API keys to manage. Prefer your own? Add a key in **Settings > Providers**, or point it at Ollama and run fully local. The UI speaks 25 languages, auto-detected from your system.

### Updates

One click, and it's done. Nebo checks for new versions in the background, and when
you accept, it updates itself and restarts. Your employees, their memory, your
workflows, and your settings all carry over. No reinstall, no reconfiguration, no
rebuilding the setup you already got working.

### Or skip the install

Launch a Nebo in Nebo Cloud from [neboai.com/cloud](https://neboai.com/cloud): same
employees, same roster, always on, nothing to host. Free for 7 days, no card. Start on
your computer and move to the cloud later, or run both. Your data stays yours either way.

## Multi-Provider

Nebo works with the model you prefer:

- **Anthropic** (Claude) — streaming, tool calls, extended thinking
- **OpenAI** (GPT) — streaming, tool calls
- **Google Gemini** — streaming, tool calls
- **Ollama** — local models, no API key needed
- **DeepSeek** — streaming, tool calls via OpenAI-compatible API
- **CLI wrappers** — `claude`, `gemini`, `codex` commands
- **Linked agents** — an employee hired through Nebo Link runs on its own agent (OpenClaw, Hermes, Claude Code, Codex, and more)

Configure providers via the Web UI or `models.yaml` in your data directory.

## Built-in Capabilities

| Domain | What it does |
|--------|-------------|
| **File** | Read, write, edit, search files and code |
| **Shell** | Execute commands, manage processes, background tasks |
| **Web** | Fetch pages, search the web, full browser automation |
| **Memory** | Store and recall facts, preferences, project context |
| **Tasks** | Hand work to parallel helpers, schedule recurring jobs |
| **Communication** | Email, texts, and messages between your bots via NeboAI |

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
| `ai` | Provider trait + implementations (Anthropic, OpenAI, Gemini, Ollama, CLI, linked) |
| `tools` | Tool registry, policy, domain tools (STRAP pattern), skills loader |
| `agent` | Runner, session, memory, compaction, advisors, search, steering |
| `server` | Axum HTTP server, handlers, WebSocket, middleware |
| `mcp` | MCP bridge, client, AES-256-GCM encryption |
| `napp` | App platform (manifest validation, sandbox, signing) |
| `workflow` | Workflow execution engine |
| `browser` | Chrome/CDP management, snapshot, native host |
| `comm` | Binary wire protocol, loopback transport, ULID |
| `notify` | System notifications |
| `updater` | One-click self-update, state preserved |
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

## App Platform

Nebo has a sandboxed app platform. Developers build `.napp` packages that extend Nebo with new tools, channels, and integrations.

- **Sandboxed** — apps run in isolated directories with gRPC over Unix sockets
- **Deny-by-default permissions** — apps only access what their manifest declares
- **Signed** — ED25519 signature verification for every app binary and manifest
- **Compiled-only** — only native binaries accepted (Go, Rust, C, Zig). No interpreted languages.
- **Distributed via NeboAI** — install apps from the marketplace or publish them privately to your account

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

Channel apps are distributed through NeboAI. Every channel reaches the same employee, with the same memory and context.

## NeboAI

[NeboAI](https://neboai.com) is the marketplace that stocks the workforce:

- **Hiring** — ready-to-work roles, and the skills, tools, workflows, and apps they're built from — created by the community
- **Cloud integrations** — pre-built connectors like My Cloud (unified Google Workspace access: Gmail, Drive, Sheets, Docs, Contacts)
- **Bots that work together** — your bots can hand work to each other across your account
- **Secure transport** — WebSocket-based binary protocol with JWT authentication

Nebo is open source and works without NeboAI: bring your own provider key, or run a local model with [Ollama](https://ollama.com) fully offline. NeboAI is opt-in.

## Security

Eight layers of defense, each enforced in code — not by prompts. See [SECURITY.md](SECURITY.md) for the full architecture and audit trail.

- Hard safeguards block destructive operations unconditionally (admin rights on your computer, disk formatting, system paths)
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
| **Linux** | Ubuntu 24.04+, Debian 13+, or any distro with glibc 2.39+ (amd64/arm64) |

## Development

```bash
# Build
make build                           # Release CLI binary
make build-desktop                   # Tauri desktop app (builds frontend first)

# Test
cargo test                           # All workspace tests

# Run
make dev                             # Desktop dev mode with hot reload

# Frontend only
cd app && pnpm dev                   # SvelteKit dev server (port 5173)
```

## Author

**NeboAI**
- Website: [neboai.com](https://neboai.com)

## License

Nebo is licensed under the [Apache License 2.0](LICENSE). Use it, modify it, build on it — freely, with an explicit patent grant from every contributor.

Bundled and vendored third-party components retain their own licenses; see [THIRD-PARTY.md](THIRD-PARTY.md).

**Trademarks.** The Apache license covers the source code only. The *Nebo* and *NeboAI* names, logos, and brand assets (including those under `src-tauri/icons/`) are trademarks and are **not** licensed for use. You may build on the code, but not ship it under the Nebo name or brand.
