# Nebo

A personal AI that lives on your computer. You name it, shape its personality, and it learns how you think. One persistent intelligence that compounds understanding the longer it stays with you.

## Why Nebo

- **It's yours.** Runs locally on your machine. Your data stays on your computer.
- **It remembers.** Persistent memory across sessions — preferences, projects, people, patterns.
- **It grows.** The longer you use it, the more useful it becomes.
- **It works.** Browser automation, file operations, shell commands, calendar, email — one AI for your entire workflow.
- **It's extensible.** An app platform where developers build and distribute capabilities via [NeboLoop](https://neboloop.com).

## Install

### macOS

```bash
# Homebrew (recommended)
brew install nebolabs/tap/nebo

# Or download the .dmg installer from the latest release
```

### Windows

Download the installer (`Nebo-setup.exe`) from the [latest release](https://github.com/nebolabs/nebo/releases/latest).

### Linux

```bash
# Debian/Ubuntu (.deb)
# Download from the latest release, then:
sudo dpkg -i nebo_*.deb

# Or build from source
git clone https://github.com/nebolabs/nebo.git
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
| **Communication** | Inter-agent messaging via comm plugins |

Platform-specific capabilities (macOS: accessibility, calendar, contacts; Windows/Linux: desktop automation) are auto-detected.

## App Platform

Nebo has a sandboxed app platform. Developers build `.napp` packages that extend Nebo with new tools, channels, UI panels, and integrations.

- **Sandboxed** — apps run in isolated directories with gRPC over Unix sockets
- **Deny-by-default permissions** — apps only access what their manifest declares
- **Signed** — ED25519 signature verification for every app binary and manifest
- **Distributed via NeboLoop** — install apps from the marketplace or deploy privately to your loop

See [Creating Apps](docs/CREATING_APPS.md) for the developer guide. The App SDK and protocol definitions (`proto/apps/`) are licensed under Apache 2.0.

## Channels

Reach your Nebo from anywhere:

| Channel | How |
|---------|-----|
| **Web UI** | `http://localhost:27895` — the primary interface |
| **CLI** | `nebo chat` — terminal chat mode |
| **Telegram** | Create bot via @BotFather, add token in Settings |
| **Discord** | Create application, add bot token in Settings |
| **Slack** | Create app with Socket Mode, add tokens in Settings |

All channels route to the same agent with the same memory and context.

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

Nebo core is source-available under the [Elastic License 2.0 (ELv2)](LICENSE). You can read, use, and modify the code — you cannot offer it as a competing product or managed service.

The App SDK, protocol definitions (`proto/apps/`), and reference apps are licensed under [Apache 2.0](https://www.apache.org/licenses/LICENSE-2.0). Build freely on our platform.
