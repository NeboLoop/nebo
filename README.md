# Nebo

A personal AI companion that lives on your computer. You name it, shape its personality, and it learns how you think. One persistent intelligence that compounds understanding the longer it stays with you.

## Why Nebo

- **It's yours.** Runs locally on your machine. Your data stays on your computer.
- **It remembers.** Persistent memory across sessions — preferences, projects, people, patterns.
- **It grows.** The longer you use it, the more useful it becomes.
- **It works.** Browser automation, file operations, shell commands, scheduling — one AI for your entire workflow.
- **It's extensible.** An app platform where developers build and distribute capabilities via [NeboLoop](https://neboloop.com).

## Install

### macOS

```bash
# Homebrew (recommended)
brew install neboloop/tap/nebo

# Or download the .dmg installer from the latest release
```

### Windows

Download the installer (`Nebo-setup.exe`) from the [latest release](https://github.com/neboloop/nebo/releases/latest).

### Linux

```bash
# Debian/Ubuntu (.deb)
# Download from the latest release, then:
sudo dpkg -i nebo_*.deb

# Or build from source
git clone https://github.com/neboloop/nebo.git
cd nebo && make build
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

## App Platform

Nebo has a sandboxed app platform. Developers build `.napp` packages that extend Nebo with new tools, channels, and integrations.

- **Sandboxed** — apps run in isolated directories with gRPC over Unix sockets
- **Deny-by-default permissions** — apps only access what their manifest declares
- **Signed** — ED25519 signature verification for every app binary and manifest
- **Custom config UI** — apps ship their own HTML/JS/CSS frontend, served by Nebo and proxied to the app binary via HTTP-over-gRPC
- **Distributed via NeboLoop** — install apps from the marketplace or deploy privately to your loop

```go
// Example: app with a config UI and a channel capability
func main() {
    app, _ := nebo.New()

    app.HandleFunc("GET /api/status", statusHandler)
    app.HandleFunc("POST /api/connect", connectHandler)
    app.RegisterChannel(NewMyChannel())

    log.Fatal(app.Run())
}
```

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

[NeboLoop](https://neboloop.com) is the optional cloud service that connects your Nebo to the outside world:

- **App Store** — discover and install apps built by the community
- **Inter-agent communication** — your Nebo can collaborate with other Nebos in your loop
- **Secure transport** — WebSocket-based binary protocol with JWT authentication

Nebo works fully offline without NeboLoop if you have a local LLM (e.g., Ollama). The cloud layer is opt-in.

## Security

Seven layers of defense, each enforced in code — not by prompts. See [SECURITY.md](SECURITY.md) for the full architecture and audit trail.

- Hard safeguards block destructive operations unconditionally (sudo, disk formatting, system paths)
- Origin tagging tracks every request source (user, comm, app, skill, system)
- Configurable tool policies with allowlists and approval flows
- App sandboxing with process isolation and ED25519 signature verification
- Compiled-only binary policy — no interpreted languages in the app platform
- All credentials encrypted at rest (AES-256-GCM)
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
make dev-setup        # First-time: install dependencies
make air              # Backend with hot reload
cd app && pnpm dev    # Frontend dev server
make test             # Run tests
make build            # Build binary (headless)
make desktop          # Build desktop app (native window + tray)
```

See [CLAUDE.md](CLAUDE.md) for full architecture documentation.

## Author

**Alma Tuck**
- Website: [almatuck.com](https://almatuck.com)
- LinkedIn: [linkedin.com/in/almatuck](https://linkedin.com/in/almatuck)
- X: [@almatuck](https://x.com/almatuck)

## License

Nebo is licensed under the [Apache License 2.0](LICENSE). Use it, modify it, build on it — freely.
