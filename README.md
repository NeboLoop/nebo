# Nebo

Your personal AI assistant. Install skills, tools, workflows, and roles to make it smarter.

Nebo lives on your computer — you name it, shape its personality, and it learns how you think. One persistent intelligence that compounds understanding the longer it stays with you. Extend it with capabilities from the [NeboLoop marketplace](https://neboloop.com) or build your own.

## Why Nebo

- **It's yours.** Runs locally on your machine. Your data stays on your computer.
- **It remembers.** Persistent memory across sessions — preferences, projects, people, patterns.
- **It grows.** The longer you use it, the more useful it becomes. Install skills, tools, workflows, and roles from the marketplace to expand what it can do.
- **It works.** Browser automation, file operations, shell commands, scheduling — one AI for your entire workflow.
- **It's extensible.** A sandboxed app platform where developers build and distribute capabilities via [NeboLoop](https://neboloop.com).

## Install

### macOS

```bash
# Homebrew (recommended)
brew install neboloop/tap/nebo

# Or download the .dmg installer from the latest release
```

### Windows

Download the installer (`Nebo-setup.exe`) from the [latest release](https://github.com/AltMagick/nebo/releases/latest).

### Linux

```bash
# Debian/Ubuntu (.deb)
# Download from the latest release, then:
sudo dpkg -i nebo_*.deb

# Or build from source
git clone https://github.com/AltMagick/nebo.git
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

Web UI runs at `http://localhost:27895`. Add your API key in **Settings > Providers**.

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
| **Communication** | Inter-agent messaging via NeboLoop |

Platform-specific capabilities (macOS: accessibility, calendar, contacts; Windows/Linux: desktop automation) are auto-detected.

## Architecture

Nebo-rs is a full Rust rewrite of the original Go implementation, built for performance, safety, and a smaller binary.

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
| `cli` | CLI entrypoint |

### Key Technology Choices

- **Axum 0.8** — async HTTP framework with tower middleware
- **SQLite** via rusqlite + r2d2 connection pool (WAL mode)
- **Tauri 2** — desktop app with native window + system tray
- **tokio** — async runtime
- **reqwest** — HTTP client with SSE streaming
- **rust-embed** — SPA static assets embedded in binary

## Local Inference

Nebo supports local model inference through two paths:

### Ollama (recommended)

No build-time dependencies. Install [Ollama](https://ollama.com), configure it as a provider in Settings, and go.

### Embedded GGUF via llama.cpp (advanced)

For users who want embedded inference without a separate Ollama process, Nebo includes an optional `local-inference` feature flag that compiles llama.cpp FFI bindings directly into the binary:

```bash
cargo build --features local-inference
```

This requires the llama.cpp C library at build time. The feature flag keeps the default build fast and portable — `cargo build` without the flag produces a binary with no native C dependencies beyond SQLite.

The `local_models` module provides a model catalog with download management, SHA256 verification, and resume support for GGUF files from Hugging Face.

## App Platform

Nebo has a sandboxed app platform. Developers build `.napp` packages that extend Nebo with new tools, channels, and integrations.

- **Sandboxed** — apps run in isolated directories with gRPC over Unix sockets
- **Deny-by-default permissions** — apps only access what their manifest declares
- **Signed** — ED25519 signature verification for every app binary and manifest
- **Compiled-only** — only native binaries accepted (Go, Rust, C, Zig). No interpreted languages.
- **Distributed via NeboLoop** — install apps from the marketplace or deploy privately to your loop

See [Creating Apps](docs/CREATING_APPS.md) for the developer guide.

## Channels

Reach your Nebo from anywhere:

| Channel | How |
|---------|-----|
| **Web UI** | `http://localhost:27895` — the primary interface |
| **CLI** | `nebo chat` — terminal chat mode |
| **Telegram** | Install the Telegram channel app from the App Store |
| **Discord** | Install the Discord channel app from the App Store |
| **Slack** | Install the Slack channel app from the App Store |

Channel apps are distributed through NeboLoop. All channels route to the same agent with the same memory and context.

## NeboLoop

[NeboLoop](https://neboloop.com) is the AI assistant marketplace that powers Nebo's extensibility:

- **Marketplace** — discover and install skills, tools, workflows, roles, and apps built by the community
- **Cloud integrations** — pre-built connectors like My Cloud (unified Google Workspace access: Gmail, Drive, Sheets, Docs, Contacts)
- **Inter-agent communication** — your Nebo can collaborate with other Nebos in your loop
- **Secure transport** — WebSocket-based binary protocol with JWT authentication

Nebo works fully offline without NeboLoop if you have a local LLM (e.g., Ollama). The marketplace is opt-in.

## Security

Seven layers of defense, each enforced in code — not by prompts. See [SECURITY.md](SECURITY.md) for the full architecture and audit trail.

- Hard safeguards block destructive operations unconditionally (sudo, disk formatting, system paths)
- Origin tagging tracks every request source (user, comm, app, skill, system)
- Configurable tool policies with allowlists and approval flows
- App sandboxing with process isolation and ED25519 signature verification
- Compiled-only binary policy — no interpreted languages in the app platform
- All credentials encrypted at rest (AES-256-GCM) with OS keychain-backed master key
- Network security: JWT auth, CSRF protection, rate limiting, security headers

## System Requirements

| Platform | Requirements |
|----------|-------------|
| **macOS** | macOS 13+ (Apple Silicon or Intel) |
| **Windows** | Windows 10+ (64-bit) |
| **Linux** | Ubuntu 22.04+ or equivalent (amd64/arm64) |

Nebo uses local models via Ollama for embeddings and background tasks. These are auto-pulled on first run (~4 GB).

## Development

```bash
# Build
cargo build                          # Debug build
cargo build --release                # Release build
cargo build --features local-inference  # With embedded llama.cpp

# Test
cargo test                           # All workspace tests (330+)

# Run
cargo run -p nebo-server             # Headless server mode

# Desktop (Tauri)
cd src-tauri && cargo tauri dev      # Desktop dev mode with hot reload

# Frontend
cd ../../app && pnpm dev             # SvelteKit dev server (port 5173)
```

## Author

**Alma Tuck**
- Website: [almatuck.com](https://almatuck.com)
- LinkedIn: [linkedin.com/in/almatuck](https://linkedin.com/in/almatuck)
- X: [@almatuck](https://x.com/almatuck)

## License

Nebo is licensed under the [Apache License 2.0](LICENSE). Use it, modify it, build on it — freely.
