# PRD: Nebo Code — Terminal Coding Agent

**Status:** Draft v2 (implementation-ready)  
**Author:** Nebo product/engineering  
**Created:** 2026-07-06  
**Updated:** 2026-07-06  
**Target release:** v0.13.x (phased)  
**Command surface:** `nebo code` — Grok-parity terminal agent (TUI + headless + subcommands)  
**Benchmark captured:** `grok --help` v0.2.87 (2026-07-06)  
**Completeness:** Strategic + nested CLI spec + schemas + GA gates (see § Decisions, § GA Criteria, Appendix E)

---

## Executive Summary

Nebo already runs a capable agentic backend — `Runner`, STRAP tools, memory, skills, plugins, sub-agents, permissions, and MCP — but the terminal experience is an afterthought. `nebo chat` bypasses the agent loop entirely (raw provider stream, no tools). Power users who live in the terminal reach for Grok Build, Claude Code, or Codex instead.

**Nebo Code** closes that gap: a first-class terminal coding agent that wires the existing Nebo brain into a Grok-quality CLI experience. The differentiation is not “another coding CLI” — it is a **coding companion** with persistent memory, marketplace skills/plugins, multi-provider routing, and the same agent that powers the desktop app.

This PRD treats **experience reverse-engineering as a first-class input**, not a shortcut. Grok Build’s UX (TUI affordances, turn semantics, diff presentation, permission flow, session resume) is the benchmark. Implementation reuses Nebo’s Rust stack and open standards (ACP, MCP); we do not decompile or fork xAI’s binary.

---

## Problem Statement

### Today

| Surface | What it does | Gap |
|---------|--------------|-----|
| `nebo` / `nebo serve` | Desktop + HTTP server | Requires browser for rich chat |
| `nebo chat` | One-shot or REPL text | **No tools, no memory, no agent loop** |
| `nebo mcp serve` | Exposes Nebo tools to *external* hosts (Cursor, Claude Desktop) | Nebo is a tool server, not the primary agent |
| `nebo test run` | Full agent loop over WebSocket | Internal harness only; no user-facing UX |

Developers who want “AI in the repo” either:

1. Run Nebo desktop and context-switch out of the terminal, or  
2. Install a separate coding CLI that does not share Nebo memory, skills, or marketplace artifacts.

### Desired

One binary, one brain — **Grok-shaped CLI surface**, Nebo brain underneath:

```bash
nebo code                                    # Interactive TUI (default)
nebo code "fix the bug"                      # TUI with initial prompt (positional)
nebo code -p "fix the flaky test"            # Headless single-turn
nebo code --worktree=feat "build this"       # New git worktree + TUI
nebo code agent stdio                        # ACP for Zed / Neovim
nebo code sessions list                      # Session management
nebo code inspect                            # Discovery / config dump
```

Same session keys, same memory, same skills — whether the user is in the desktop app, terminal, or an IDE via ACP.

> **Surface decision:** Grok exposes everything at the top-level binary (`grok`). Nebo nests under `nebo code` to avoid breaking `nebo serve`, `nebo doctor`, etc. Phase 4 may add a `nebocode` shim alias if muscle memory demands it.

---

## Grok CLI Surface Inventory (Source of Truth)

Captured from `grok --help` and subcommand help. This section is the **parity checklist** — every row must have a Nebo disposition before GA.

### Top-level invocation

```
grok [OPTIONS] [PROMPT] [COMMAND]
```

| Grok pattern | Nebo equivalent | Phase |
|--------------|-----------------|-------|
| `grok` | `nebo code` | 2 |
| `grok "fix the bug"` | `nebo code "fix the bug"` | 2 |
| `grok -p "..."` | `nebo code -p "..."` | 0 |
| `grok agent stdio` | `nebo code agent stdio` | 3 |
| `grok inspect` | `nebo code inspect` | 1 |
| `grok sessions list` | `nebo code sessions list` | 2 |

### Global flags — full parity matrix

| Flag | Grok behavior | Nebo mapping | Phase | Notes |
|------|---------------|--------------|-------|-------|
| `[PROMPT]` positional | Initial TUI prompt | `nebo code [PROMPT]` | 2 | |
| `--agent <NAME>` | Agent name or definition file | `--agent` → Nebo agent id or `AGENT.md` path | 1 | Uses `agent_id` + napp loader |
| `--agents <JSON>` | Inline subagent defs | `--agents` JSON → spawn config | 4 | Maps to orchestrator spawn |
| `--allow <RULE>` | Permission allow (Claude `--allowedTools`) | `--allow` glob rules | 2 | Extend policy engine |
| `--always-approve` | Auto-approve all tools | `--always-approve` → `full_access` | 0 | |
| `--best-of-n <N>` | Parallel N runs, pick best (headless) | `--best-of-n` via `agent/testing` optimizer | 5 | Reuse experiment harness |
| `-c, --continue` | Resume latest session for cwd | `-c, --continue` | 0 | |
| `--check` | Self-verification loop (headless) | `--check` appends verify prompt + test pass | 4 | Like Grok `--verify` |
| `--cwd <CWD>` | Working directory | `--cwd` → `allowed_paths` | 0 | |
| `--debug` | Debug logging | `--debug` → `RUST_LOG=debug` | 0 | |
| `--debug-file <FILE>` | Log to file | `--debug-file` | 1 | |
| `--deny <RULE>` | Permission deny | `--deny` glob rules | 2 | |
| `--disable-web-search` | Strip web tools | `--disable-web-search` | 0 | Tool denylist |
| `--disallowed-tools <TOOLS>` | Tool denylist | `--disallowed-tools` STRAP names | 0 | |
| `--effort <LEVEL>` | low/medium/high/xhigh/max | `--effort` → provider metadata / max_turns | 4 | Nebo: map to thinking + iteration budget |
| `--experimental-memory` | Cross-session memory on | **Default on** for Nebo (core value) | 0 | `--no-memory` to disable |
| `--fork-session` | Resume into new session id | `--fork-session` | 2 | With `--resume`/`--continue` |
| `--json-schema <SCHEMA>` | Structured JSON output | `--json-schema` + structured agent | 4 | `agent/structured.rs` |
| `--leader-socket <PATH>` | Custom leader socket | `--leader-socket` | 5 | See leader architecture |
| `-m, --model <MODEL>` | Model override | `-m, --model` fuzzy resolve | 0 | `agent/selector.rs` |
| `--max-turns <N>` | Agent turn cap | `--max-turns` → `max_iterations` | 0 | |
| `--minimal` | Scrollback-native TUI | `--minimal` inline renderer | 5 | Experimental |
| `--no-alt-screen` | Inline TUI (no alt screen) | `--no-alt-screen` | 3 | REPL fallback |
| `--no-memory` | Disable memory | `--no-memory` → `skip_memory_extract` + no recall | 0 | |
| `--no-plan` | Disable plan mode | `--no-plan` | 1 | Default plan on for writes |
| `--no-subagents` | Disable subagent spawn | `--no-subagents` | 1 | |
| `--oauth` | OAuth on welcome | N/A — Nebo uses provider keys / NeboAI auth | — | `nebo code login` differs |
| `--output-format <FMT>` | plain/json/streaming-json | `--output-format` same three | 0 | |
| `-p, --single <PROMPT>` | Headless single-turn | `-p, --single` | 0 | |
| `--permission-mode <MODE>` | default/acceptEdits/auto/dontAsk/bypassPermissions/plan | `--permission-mode` enum | 2 | Maps to `full_access` + plan |
| `--prompt-file <PATH>` | Headless prompt from file | `--prompt-file` | 1 | |
| `--prompt-json <JSON>` | Prompt as content blocks | `--prompt-json` (images, etc.) | 3 | |
| `-r, --resume [<ID>]` | Resume session | `-r, --resume` | 0 | |
| `--reasoning-effort <EFFORT>` | Reasoning model effort | `--reasoning-effort` → `enable_thinking` | 1 | |
| `--restore-code` | Checkout session commit on resume | `--restore-code` via git snapshot | 4 | Needs session→commit index |
| `--rules <RULES>` | Append system rules | `--rules` → steering snippet | 0 | |
| `-s, --session-id <UUID>` | New session with fixed UUID | `-s, --session-id` | 1 | Fork naming with `--fork-session` |
| `--sandbox <PROFILE>` | OS sandbox profile | `--sandbox` → VM tool / future seatbelt | 4 | Nebo has `vm` STRAP |
| `--system-prompt-override` | Full system prompt replace | `--system-prompt-override` | 1 | Headless + power users |
| `--tools <TOOLS>` | Tool allowlist | `--tools` STRAP allowlist | 0 | |
| `--verbatim` | Send prompt as-is | `--verbatim` skip template wrap | 2 | |
| `-w, --worktree [<NAME>]` | Start in new git worktree | `-w, --worktree` | 3 | |
| `--worktree-ref <REF>` | Base ref for worktree | `--worktree-ref` / `--ref` | 3 | |
| `-h, --help` | Help | `-h, --help` | 0 | |
| `-v, --version` | Version | `nebo --version` (existing) | 0 | |

### Subcommands — full parity matrix

| Subcommand | Grok behavior | Nebo `nebo code <cmd>` | Phase | Notes |
|------------|---------------|------------------------|-------|-------|
| `agent stdio` | ACP JSON-RPC stdio | `agent stdio` | 3 | `agent-client-protocol` crate |
| `agent headless` | WS relay headless | `agent headless` attach to server WS | 3 | For SDK consumers |
| `agent serve` | Agent as WS server | **Defer** — Nebo has `nebo serve` | — | Document mapping |
| `agent leader` | Shared leader process | `agent leader` | 5 | Long-lived `AppState` daemon |
| `completions` | Shell completions | `completions` bash/zsh/fish | 2 | clap completions |
| `dashboard` | Agent dashboard TUI at startup | **Stretch** — link to desktop `/activity` | 5 | Or terminal dashboard |
| `export` | Session → Markdown | `export <session-id>` | 2 | |
| `import` | Import sessions | `import` from Grok/Claude formats | 5 | Migration tooling |
| `inspect` | Config discovery dump | `inspect [--json]` | 1 | High priority |
| `leader` | Manage leader processes | `leader list/info/kill/profile` | 5 | Matches Grok nested cmds |
| `login` | Sign in | `login` → provider / NeboAI OAuth | 1 | Reuse `auth` crate |
| `logout` | Clear credentials | `logout` | 1 | |
| `mcp` | Manage MCP configs | `mcp list/add/remove/doctor` | 2 | Merge with settings MCP |
| `memory` | Cross-session memory mgmt | `memory clear` (+ Nebo extras: `search`, `list`) | 2 | Grok: `clear` only; Nebo adds DB-backed search |
| `models` | List models, exit | `models` | 0 | Reuse provider list |
| `plugin` | Plugin + marketplace mgmt | `plugin list/install/.../marketplace` | 3 | See Appendix E |
| `sessions` | list/search/delete | `sessions list/search/delete` | 2 | |
| `setup` | Fetch managed config | `setup` onboarding wizard | 1 | Extends `nebo onboard` |
| `trace` | Export/upload traces | `trace export` | 4 | Reuse `agent/testing/trace` |
| `update` | Self-update | `nebo update` (existing updater) | — | Already on `nebo` top-level |
| `version` | Print version | top-level `nebo --version` | 0 | |
| `worktree` | list/show/rm/gc/db | `worktree` git worktree manager | 3 | |
| `wrap` | OSC 52 clipboard wrapper | `wrap` for SSH sessions | 5 | Nice for remote dev |

### `grok agent` flags (ACP / SDK path)

| Flag | Nebo equivalent | Phase |
|------|-----------------|-------|
| `--reauth` | `--reauth` force provider re-login | 2 |
| `--agent-profile <PATH>` | `--agent-profile` | 1 |
| `--plugin-dir <DIR>` | `--plugin-dir` trusted ephemeral plugin | 3 |
| `--leader` / `--no-leader` | Connect to `nebo code agent leader` | 5 |
| `--leader-socket <PATH>` | Unix socket path | 5 |

### Leader architecture (Grok pattern — Phase 5)

Grok runs a **leader process** (`~/.grok/leader.sock`) so multiple clients (TUI, IDE, headless) share one agent backend. Nebo equivalent:

```
nebo code agent leader          # foreground or --background
nebo code --leader              # TUI attaches to leader
nebo code agent stdio --leader  # ACP attaches to leader
```

Benefits: one SQLite pool, one `Runner`, shared session state with desktop when co-located. Implementation: Unix domain socket + JSON-RPC shim to internal `AppState`, or require `nebo serve` as the leader (simpler Phase 5a).

### Permission modes (Claude Code parity)

| Mode | Behavior | Nebo implementation |
|------|----------|---------------------|
| `default` | Prompt on sensitive tools | Current approval gate |
| `acceptEdits` | Auto-approve file edits only | Policy: auto `os.write`/`os.edit`, prompt shell |
| `auto` | Auto-approve with audit log | `full_access` + log |
| `dontAsk` | Deny unless pre-allowed | Deny-by-default + `--allow` rules |
| `bypassPermissions` | Same as `--always-approve` | `full_access: true` |
| `plan` | Plan-only until approved | `plan_mode: true` |

### Effort levels

Grok: `low | medium | high | xhigh | max`. Nebo maps to:

- Provider thinking budget (`enable_thinking`, extended thinking tokens)
- `max_iterations` scaling
- Subagent parallelism cap
- Optional: model routing (fast model for low, flagship for max)

---

## Why Experience Reverse-Engineering Matters

Grok Build’s **implementation** is closed source (Rust binary). Its **experience** is largely observable and documented. For Nebo, the experience *is* the product in the terminal — parity on architecture alone produces a capable but forgettable tool.

### Legitimate experience sources (in scope for this PRD)

| Source | What we extract |
|--------|-----------------|
| `~/.grok/README.md` | Slash commands, shortcuts, headless flags, session layout, tool IDs |
| Headless observation | `grok -p "..." --output-format json` → event stream contract |
| `grok inspect --json` | Discovery model (skills, agents, MCP, hooks) |
| [ACP specification](https://agentclientprotocol.com) | IDE integration protocol (Grok uses `agent-client-protocol` crate) |
| Session files on disk | `~/.grok/sessions/<cwd>/<id>/` → persistence UX patterns |
| Public marketing / release notes | Plan mode, diff-first edits, sub-agent panels |

### Explicitly out of scope

- Decompiling or redistributing xAI binaries  
- Coupling to xAI API or Grok models  
- Pixel-perfect UI clone (Nebo has its own visual identity)

### Experience principles (benchmarked from Grok)

1. **Turn-native** — User thinks in “turns” (prompt → tool loop → result), not chat bubbles disconnected from side effects.  
2. **Diff-first edits** — File changes are shown as reviewable diffs before/after apply; user can reject or rewind.  
3. **Visible tool work** — Every tool call shows name, target, and streaming output; silence is a bug.  
4. **Permission as rhythm** — Approvals are fast (y/n), batched when safe, skippable with explicit `--yolo` / `/always-approve`.  
5. **Session = project + thread** — Resume by cwd; session ID is stable across headless and TUI.  
6. **@file is muscle memory** — Fuzzy picker, line ranges, `!` for dotfiles; no manual paste of file contents.  
7. **Plan before execute** — Complex tasks get an approvable plan; small tasks skip straight to work.  
8. **Sub-agents are visible** — Parallel child tasks show status panels, not hidden background work.  
9. **Inspectability** — `nebo code inspect` answers “what will this session see?” before spending tokens.  
10. **Zero server ceremony** — Default: embedded agent in the CLI process; optional attach to running `nebo serve`.

---

## Product Vision

> **Nebo Code** — the terminal interface to your companion’s brain. Same memory, same skills, same marketplace — in the directory where you work.

### Positioning

| Competitor | Position | Nebo Code angle |
|------------|----------|-----------------|
| Grok Build | xAI-native coding agent, single provider | Multi-provider, local-first data, marketplace extensions |
| Claude Code | Anthropic-native, IDE-first | Companion memory + skills + workflows + desktop continuity |
| Cursor Agent CLI | Editor-ecosystem | Standalone terminal + ACP; works without Cursor |
| `nebo mcp serve` | Nebo as tool backend | Nebo as **primary** agent with optional MCP *client* |

### Non-goals

- Replacing the desktop app  
- Becoming a general-purpose shell  
- Requiring cloud / NeboAI account for local-only usage  

---

## Target Users

1. **Nebo power users** — Already run desktop; want terminal parity without losing memory/skills.  
2. **Terminal-native developers** — Live in tmux/kitty; want Claude Code–class UX with provider choice.  
3. **IDE integrators** — Use Zed/Neovim/Emacs via ACP; want Nebo as the agent backend.  
4. **Automation authors** — CI, pre-commit, cron scripts via headless `-p` + `--output-format json`.  
5. **Non-dev professionals** (stretch) — Lawyers/realtors who already use Nebo; simplified `nebo code` with stricter permissions and guided plan mode.

---

## Modes of Operation

### 1. Interactive TUI (`nebo code`)

Default when no subcommand. Matches `grok` with no flags.

```bash
nebo code
nebo code "Review artifact_updates.rs"
nebo code --cwd ~/projects/nebo --worktree=feat-auth "implement OAuth refresh"
nebo code --no-alt-screen          # inline mode (Phase 3)
nebo code --minimal                # scrollback-native (Phase 5)
nebo code --dashboard              # subcommand: dashboard at startup (Phase 5)
```

**Exit codes:** `0` success, `1` agent/tool error, `130` user interrupt.

### 2. Headless (`nebo code -p` / `-p`)

Matches `grok -p` / `--single`.

```bash
nebo code -p "Run tests and summarize failures"
nebo code -p "..." --output-format json
nebo code -p "..." --output-format streaming-json
nebo code -p "..." -c                    # continue latest cwd session
nebo code -p "..." -r abc-uuid           # resume specific session
nebo code -p "..." --check               # self-verify loop (Phase 4)
nebo code -p "..." --best-of-n 3         # parallel attempts (Phase 5)
nebo code -p "..." --json-schema '{...}' # structured output (Phase 4)
nebo code --prompt-file task.md --always-approve
```

### 3. Agent modes (`nebo code agent`)

Matches `grok agent` subcommands.

```bash
nebo code agent stdio              # ACP for Zed/Neovim (Phase 3)
nebo code agent headless           # WS client to running nebo serve (Phase 3)
nebo code agent leader             # shared backend daemon (Phase 5)
nebo code agent stdio --leader     # ACP via leader socket
```

ACP uses `agent-client-protocol` crate. Session keys: `acp:<sessionId>` (`crates/agent/src/keyparser.rs`).

### 4. Management commands

```bash
nebo code inspect [--json]
nebo code models
nebo code sessions list|search|delete
nebo code memory clear|list|search
nebo code mcp list|add|remove
nebo code plugin list|install
nebo code worktree list|show|rm|gc
nebo code export <session-id> -o transcript.md
nebo code login | logout
nebo code setup
nebo code completions bash|zsh|fish
```

### 5. Attach / leader (optional)

```bash
nebo code --server localhost:27895     # attach to running nebo serve
nebo code --leader                     # attach to leader socket (Phase 5)
```

Default remains **embedded** for zero-setup. Auto-detect running server/leader and print hint.

---

## Experience Specification (Grok-Informed)

### TUI layout

```
┌─────────────────────────────────────────────────────────────────┐
│ nebo code · nebo v0.13 · agent:default · model:claude-sonnet-4  │
│ cwd: ~/workspaces/nebo/nebo          [plan] [yolo:off] [turn 3]   │
├─────────────────────────────────────────────────────────────────┤
│ CONVERSATION + TOOL STREAM                                        │
│                                                                   │
│ ❯ fix artifact update history to record artifact names           │
│                                                                   │
│ ◆ Thought 2.1s                                                    │
│ I'll read the artifact_updates handler and the diff you have...  │
│                                                                   │
│ ┌─ os · read_file ─────────────────────────────────────────────┐ │
│ │ crates/server/src/artifact_updates.rs                        │ │
│ └──────────────────────────────────────────────────────────────┘ │
│                                                                   │
│ ┌─ diff · artifact_updates.rs ─────────────────── [approve][n] ┐ │
│ │ - ""                                                          │ │
│ │ + artifact.name.as_deref().unwrap_or("")                      │ │
│ └──────────────────────────────────────────────────────────────┘ │
├─────────────────────────────────────────────────────────────────┤
│ TASKS (Ctrl+T)          │ SUBAGENTS (when active)                 │
│ ☐ read handler          │ explore · grep usages      [running]    │
│ ☐ patch 3 sites         │                                           │
├─────────────────────────────────────────────────────────────────┤
│ ❯ _                                          [@] [multiline:off] │
│ enter:send  ^C:cancel  ^O:yolo  ^R:history  /help                │
└─────────────────────────────────────────────────────────────────┘
```

### Keyboard shortcuts (parity target)

| Key | Action |
|-----|--------|
| `Enter` | Send message |
| `Shift+Enter` / `Alt+Enter` | Newline |
| `Ctrl+C` / `Esc` | Cancel current turn / tool |
| `Ctrl+D` / `Ctrl+Q` | Quit (confirm if turn in flight) |
| `Ctrl+O` | Toggle always-approve (yolo) |
| `Ctrl+T` | Toggle task/subagent panel |
| `Ctrl+R` | Prompt history search |
| `Ctrl+G` | Background foreground task |
| `Ctrl+P` | Debug panel (events, session id, token usage) |

Configurable via `~/.nebo/code.toml` `[shortcuts]` (Grok-compatible keys).

### Slash commands (full inventory)

Grok TUI slash commands with Nebo disposition. **P0** = Phase 1–2, **P4** = later.

| Command | Alias | Grok behavior | Nebo | Phase |
|---------|-------|---------------|------|-------|
| `/model <name>` | `/m` | Switch model | Switch provider+model | P0 |
| `/new` | | Clear context, new session | Same | P0 |
| `/load [workspace] [session]` | `/resume` | Load prior session | Same | P1 |
| `/rewind <prompt>` | | Restore files to prompt point | Git + snapshot rewind | P4 |
| `/compact [context]` | | Compact history | Reuse `agent/compaction.rs` | P1 |
| `/always-approve [on\|off]` | `/yolo` | Toggle auto-approve | `full_access` toggle | P0 |
| `/multiline` | `/ml` | Toggle multiline input | Same | P2 |
| `/memory [workspace\|global] <text>` | | Append memory file | Nebo memory tool (default on) | P1 |
| `/flush` | | Flush session → memory | Memory consolidation trigger | P2 |
| `/skills [name]` | | List / inject skill | Marketplace + local skills | P1 |
| `/plugins [list\|reload\|trust]` | `/plugin` | Plugin mgmt | Nebo plugin trust flow | P3 |
| `/hooks-list` | | Show loaded hooks | Same | P4 |
| `/hooks-trust` | | Trust project hooks | Workspace trust file | P4 |
| `/hooks-add <path>` | | Add hook path | Same | P4 |
| `/inspect` | | Config discovery | `nebo code inspect` inline | P1 |
| `/doctor` | | Diagnostics | Reuse `nebo doctor` | P1 |
| `/plan [on\|off]` | | Toggle plan mode | `plan_mode` | P2 |
| `/feedback [message]` | | Send feedback | **Defer** (no telemetry channel) | — |
| `/exit` | `/quit` | Exit TUI | Same | P0 |

**Nebo-only (not in Grok):** `/agent <id>`, `/marketplace <code>`, `/workflow` — stretch Phase 4+.

### @file references

```
@crates/cli/src/main.rs
@crates/cli/src/main.rs:160-200
@crates/server/
@!.env                    # bypass gitignore for dotfiles
```

Implementation: fuzzy picker over cwd (respect `respect_gitignore` config, default true). Selected files attach as multimodal context or inlined snippets with path headers.

### Plan mode

For prompts classified as high-impact (file writes, shell, multi-step) or when `/plan on`:

1. Agent emits structured plan (markdown checklist).  
2. TUI shows plan panel; user approves, edits, or rejects.  
3. On approve, runner executes with `plan_mode` cleared for execution phase.

Maps to existing `RunRequest.plan_mode` + plan approval events in the runner.

### Permission UX

| Mode | CLI flag | Session | Behavior |
|------|----------|---------|----------|
| Default | | | Prompt per sensitive tool (shell, write, network) |
| Always approve | `--always-approve` | `/yolo on` | `full_access: true` |
| Read-only | `--tools os,web` + deny writes | | Static tool allowlist |
| Rules | `--allow` / `--deny` | | Glob rules on tool args (Grok-style) |

Terminal prompt format (single keystroke):

```
Allow os.write_file crates/server/src/foo.rs? [y/N/a(all)]
```

### Tool presentation map (Nebo STRAP → user-facing)

| User label | STRAP / internal | Grok analogue |
|------------|------------------|---------------|
| read file | `os.read` | `read_file` |
| edit file | `os.write` / `os.edit` | `search_replace` |
| grep | `os.grep` | `grep_search` |
| list dir | `os.list` | `list_dir` |
| shell | `os.shell` | `bash` |
| web search | `web.search` | `web_search` |
| fetch url | `web.fetch` | `web_fetch` |
| delegate | `agent.spawn` | `task` |
| memory | `bot.memory` | `memory_*` |
| skill | `skill.invoke` | `/skills` injection |

Headless `--tools` / `--disallowed-tools` accept STRAP domain names (`os`, `web`, `agent`, …) and meta aliases (`bash` → `os.shell`).

### Session persistence

**Layout** (mirrors Grok’s discoverability, Nebo paths):

```
~/.nebo/sessions/<url-encoded-cwd>/<session-id>/
  summary.json           # title, model, timestamps, agent_id
  events.jsonl           # canonical event stream (WS-equivalent)
  rewind.jsonl           # optional file snapshots for /rewind
  subagents/             # child session metadata
```

**Resume:**

```bash
nebo code --continue
nebo code --resume <session-id>
nebo code -p "..." --session-id <id>
```

TUI `/new` creates fresh session under same cwd without deleting history.

### Headless JSON event schema

Newline-delimited JSON (`--output-format json` or `streaming-json`). **Schema version:** `nebo-code-events/v1`.

| Event `type` | Fields | When |
|--------------|--------|------|
| `session` | `session_id`, `cwd`, `model`, `schema_version` | Start |
| `text` | `delta` | Assistant text chunk |
| `thought` | `text`, `duration_ms?` | Reasoning block |
| `tool_start` | `tool`, `call_id`, `args` | Tool invoked |
| `tool_output` | `tool`, `call_id`, `stream`, `text` | Streaming tool stdout/stderr |
| `tool_end` | `tool`, `call_id`, `ok`, `error?` | Tool finished |
| `diff` | `path`, `patch`, `status` | File change preview (`pending\|applied\|rejected`) |
| `plan` | `markdown`, `steps[]` | Plan mode output |
| `approval_required` | `tool`, `call_id`, `prompt` | Waiting for user |
| `approval_resolved` | `call_id`, `approved` | User responded |
| `subagent_start` | `id`, `role`, `task` | Child agent spawned |
| `subagent_end` | `id`, `ok` | Child finished |
| `turn_end` | `turn`, `usage` | Turn complete |
| `done` | `exit_code` | Run finished (headless) |
| `error` | `message`, `code?` | Fatal error |

`streaming-json` emits each event as produced; `json` may buffer per-turn. Both use the same event shapes.

Example:

```jsonl
{"type":"session","schema_version":"nebo-code-events/v1","session_id":"8f3c...","cwd":"/Users/me/nebo","model":"claude-sonnet-4"}
{"type":"text","delta":"I'll read the file"}
{"type":"tool_start","tool":"os.read","call_id":"tc_1","args":{"path":"crates/cli/src/main.rs"}}
{"type":"tool_end","tool":"os.read","call_id":"tc_1","ok":true}
{"type":"turn_end","turn":1,"usage":{"input":4200,"output":180}}
{"type":"done","exit_code":0}
```

### Session file schemas

**`summary.json`**

```json
{
  "id": "uuid",
  "title": "fix artifact history",
  "cwd": "/path/to/repo",
  "cwd_encoded": "path%2Fto%2Frepo",
  "model": "claude-sonnet-4",
  "provider_id": "auth-profile-uuid",
  "agent_id": "default",
  "created_at": "2026-07-06T12:00:00Z",
  "updated_at": "2026-07-06T12:05:00Z",
  "turn_count": 3,
  "parent_session_id": null,
  "git_head": "603cbba0",
  "forked_from": null
}
```

**`events.jsonl`** — one JSON object per line; same types as headless schema plus `role`/`content` for persisted chat.

**`rewind_points.jsonl`** — `{ "prompt_index", "git_commit?", "snapshots": [{"path","before_hash"}] }`

Session key in Nebo DB (optional mirror): `code:<url-encoded-cwd>:<session-id>`.

### Permission rules (`--allow` / `--deny`)

Claude Code–compatible glob rules. Repeatable flags; deny wins over allow.

**Syntax:** `<tool>(<arg-pattern>)` or bare `<tool>`

| Rule example | Meaning |
|--------------|---------|
| `os.read` | Allow/deny all file reads |
| `os.read(crates/**)` | Scope to path glob |
| `os.shell` | All shell commands |
| `os.shell(cargo *)` | Only cargo invocations |
| `agent` | All subagent spawns |
| `agent(explore)` | Specific subagent type |
| `web` | All web tools |

Implementation: extend `tools/policy.rs` with glob matcher on tool name + serialized args JSON.

### `nebo code inspect`

Human and `--json` modes. Reports:

- Project rules (`AGENTS.md`, Nebo rules)  
- Skills (project + user + marketplace)  
- Plugins / MCP servers  
- Active agent profile  
- Tool allowlist effective for session  
- Provider/model resolution  
- `cwd`, `allowed_paths`, git root  

---

## Technical Architecture

### High-level

```
┌──────────────────────────────────────────────────────────────┐
│              nebo code · nebo code agent stdio                  │
│  ┌────────────┐  ┌─────────────┐  ┌────────────────────────┐ │
│  │ TUI (ratatui)│  │ Headless    │  │ ACP stdio server       │ │
│  │ EventRenderer│  │ stdout/json │  │ agent-client-protocol  │ │
│  └──────┬───────┘  └──────┬──────┘  └───────────┬────────────┘ │
│         └─────────────────┼─────────────────────┘              │
│                           ▼                                    │
│              ┌────────────────────────┐                        │
│              │   CodeSessionDriver    │                        │
│              │  (maps UI ↔ Runner)    │                        │
│              └────────────┬───────────┘                        │
└───────────────────────────┼──────────────────────────────────┘
                            ▼
              ┌────────────────────────┐
              │  Embedded AppState     │  OR  │ attach: HTTP/WS │
              │  Runner::run(req)      │      │ chat_dispatch   │
              └────────────────────────┘
                            ▼
              ┌────────────────────────┐
              │  STRAP tools + policy  │
              │  memory, skills, MCP   │
              └────────────────────────┘
```

### New / modified crates

| Area | Location | Notes |
|------|----------|-------|
| CLI subcommands | `crates/cli/src/code/` | `mod.rs`, `tui/`, `headless.rs`, `inspect.rs` |
| ACP server | `crates/cli/src/acp/` or `crates/acp/` | Thin wrapper over `agent-client-protocol` |
| Session store | `crates/cli/src/sessions/` | Filesystem JSONL; optional DB mirror later |
| TUI | `crates/cli/src/code/tui/` | `ratatui` + `crossterm` |
| Shared driver | `crates/cli/src/code/driver.rs` | Builds `RunRequest`, consumes `StreamEvent` |
| Server attach | reuse | `chat_dispatch::run_chat_events` over WS |

### `RunRequest` wiring (coding defaults)

```rust
RunRequest {
    session_key: format!("acp:{id}").or(coding_session_key(cwd, id)),
    origin: Origin::User,  // CLI is interactive user origin (see crates/tools/src/origin.rs)
    channel: "code",
    allowed_paths: vec![cwd.canonicalize()?],
    full_access: cli.always_approve,
    plan_mode: cli.plan || settings.plan_default,
    agent_id: cli.agent.unwrap_or_default(),
    tool_scope: Some("coding".into()),  // new scope: os+web+agent, no desktop
    prompt_mode: PromptMode::Full,
    // ...
}
```

### Embedded vs attach

| Mode | Pros | Cons |
|------|------|------|
| **Embedded (default)** | No server boot; single binary | Larger cold start; duplicate AppState if desktop also running |
| **Attach** | One runner; shared sessions with desktop | Requires server; worse first-run UX |

**Decision:** Ship embedded first; detect running server on `localhost:27895` and offer seamless attach (`--server auto`).

### Dependencies (new)

```toml
# crates/cli/Cargo.toml
ratatui = "0.29"
crossterm = "0.28"
agent-client-protocol = "0.6"
fuzzy-matcher = "0.3"   # @file picker
```

### Compatibility conventions

| Convention | Nebo support |
|------------|--------------|
| `AGENTS.md` | Merge from git root → cwd into steering (like Grok) |
| `.mcp.json` | Merge with `~/.nebo/settings.json` MCP config |
| `.agents/skills/` | Fallback path for skills (alias `~/.nebo/skills/`) |
| Claude Code paths | Read-only discovery (`~/.claude.json`, etc.) — optional, config-gated |

---

## Differentiation (Beyond Grok Parity)

Features Grok does not emphasize but Nebo already has:

1. **Multi-provider routing** — `/model` can select Ollama local, Anthropic, OpenAI, DeepSeek, CLI wrappers.  
2. **Marketplace skills/plugins** — Install codes mid-session; deps cascade.  
3. **Desktop continuity** — Same DB at `~/.nebo/data/nebo.db`; desktop chat and terminal share memory entities.  
4. **Workflow hooks** — Post-tool hooks can trigger Nebo workflows (stretch).  
5. **VM sandbox tool** — `vm` STRAP tool for isolated execution (macOS/Linux), alternative to Grok’s OS sandbox.  
6. **Companion positioning** — Memory, advisors, personality — available when user wants “my companion who codes,” not generic agent.

---

## Phased Delivery

Phases align to the **parity matrix** above. Each phase closes a slice of `grok --help`.

### Phase 0 — Headless core (2 weeks) — P0 flags

- [ ] `nebo code -p` / `--single` embedded runner  
- [ ] `-m`, `--cwd`, `--rules`, `--always-approve`, `--max-turns`  
- [ ] `--tools`, `--disallowed-tools`, `--disable-web-search`  
- [ ] `--output-format plain|json|streaming-json` (Grok name; not `--format`)  
- [ ] `-c/--continue`, `-r/--resume`  
- [ ] `--no-memory`, `--debug`  
- [ ] `nebo code models`  
- [ ] Memory **on by default** (Nebo differentiator — no `--experimental-memory` gate)  
- [ ] Deprecate `nebo chat` with redirect message  

**Verify:** `nebo code -p "cargo test -p nebo-server" --output-format json | jq .`

### Phase 1 — Interactive + discovery (2 weeks)

- [ ] Positional `[PROMPT]` for multi-turn (no alt-screen REPL first)  
- [ ] `--no-plan`, `--no-subagents`, `--reasoning-effort`  
- [ ] `--prompt-file`, `--system-prompt-override`, `--agent`  
- [ ] `nebo code inspect [--json]`  
- [ ] `nebo code login` / `logout` / `setup`  
- [ ] `nebo code sessions list` (read-only)  
- [ ] Approval prompts; `--permission-mode` basic (default, bypassPermissions)  
- [ ] Slash commands: `/model`, `/new`, `/resume`, `/yolo`, `/compact`, `/exit`  

**Verify:** `nebo code inspect` output matches expected skills/tools for nebo repo.

### Phase 2 — Full TUI + sessions (3–4 weeks)

- [ ] ratatui alt-screen TUI (default)  
- [ ] Positional prompt, @file picker + `!` dotfiles  
- [ ] Keyboard shortcuts parity table  
- [ ] Inline diff approve/reject  
- [ ] Plan mode UI + `--permission-mode plan`  
- [ ] `--permission-mode` full enum + `--allow`/`--deny`  
- [ ] `sessions search|delete`, `export`, `memory` subcommands  
- [ ] `mcp` config subcommand  
- [ ] `completions`  
- [ ] `--fork-session`, `-s/--session-id`  
- [ ] `--verbatim`  

**Verify:** Side-by-side with `grok` on: fix bug, resume session, export transcript.

### Phase 3 — ACP + worktrees + plugins (3 weeks)

- [ ] `nebo code agent stdio` (ACP)  
- [ ] `nebo code agent headless` (WS attach)  
- [ ] `--no-alt-screen` inline mode  
- [ ] `-w/--worktree`, `--worktree-ref`, `worktree` subcommand  
- [ ] `plugin` subcommand (marketplace install by code)  
- [ ] `--prompt-json` (multimodal blocks)  
- [ ] `--agent-profile`, `--plugin-dir`  
- [ ] Zed/Neovim setup docs  

**Verify:** Zed external agent completes multi-file edit; worktree session isolates branch.

### Phase 4 — Advanced agent features (3 weeks)

- [ ] `--check` self-verification loop  
- [ ] `--json-schema` structured output  
- [ ] `--restore-code` on resume  
- [ ] `--sandbox` profiles (VM STRAP integration)  
- [ ] `trace export`  
- [ ] `/rewind` + file snapshots  
- [ ] Hooks — `.nebo/hooks/` (PreToolUse, PostToolUse)  
- [ ] `--effort` levels  
- [ ] Claude Code config discovery (read-only)  
- [ ] LSP tool (optional)  

**Verify:** `nebo code -p "add test" --check` runs tests before declaring done.

### Phase 5 — Leader, best-of-n, polish (ongoing)

- [ ] `agent leader` + `--leader` + `leader` subcommand  
- [ ] `--best-of-n` via testing optimizer  
- [ ] `--minimal` scrollback renderer  
- [ ] `import` sessions (Grok/Claude migration)  
- [ ] `dashboard` TUI or deep-link to desktop  
- [ ] `wrap` OSC 52 clipboard  
- [ ] `nebocode` optional alias binary  

**Verify:** TUI + `agent stdio` + headless `-p` share leader; no duplicate runners.

---

## Success Metrics

| Metric | Target (90 days post GA) |
|--------|--------------------------|
| Weekly `nebo code` invocations (telemetry opt-in) | 20% of active CLI users |
| Session resume rate | >30% of TUI sessions use `--continue` or `/resume` |
| Desktop ↔ terminal shared memory | Measurable memory reads after terminal sessions |
| ACP adoption | ≥1 documented IDE setup; community configs |
| Dogfood | Nebo team uses `nebo code` for ≥50% of repo edits |
| Time-to-first-success | `<60s` from `brew install nebo` to successful `-p` run |

---

## Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| TUI scope explosion | Delays MVP | Phase 0/1 ship first; TUI is Phase 2 |
| Embedded AppState cold start | Slow launch | Lazy init; share pool with desktop when running |
| Permission UX in raw terminal | Users blast `--yolo` | Default safe; prominent warnings; audit log in `~/.nebo/logs/code.log` |
| Experience parity creep | Never ships | Written acceptance tests from Grok observation checklist (appendix) |
| `nebo chat` confusion | Two chat CLIs | Deprecation notice; `nebo chat` prints redirect hint |
| Provider without tool support | Broken coding loop | `nebo code doctor` validates active provider handles tools |

---

## Configuration Reference (`~/.nebo/code.toml`)

Project override: `.nebo/code.toml`. Merges with `~/.nebo/settings.json` for providers.

```toml
[cli]
auto_update = true                    # delegate to nebo updater

[models]
default = "claude-sonnet-4"           # fuzzy-resolved via agent/selector.rs
web_search = "..."                    # model for web.search tool (optional)

[ui]
max_thoughts_width = 120
theme = "dark"                        # dark | light

[features]
plan_default = true                   # plan before writes (Nebo default; Grok uses permission-mode)
lsp_tools = false
codebase_indexing = true              # defer: Phase 5
telemetry = false

[session]
auto_compact_threshold_percent = 85
load_envrc = true                     # pass .envrc to os.shell
channel = "code"                      # RunRequest.channel

[tools]
respect_gitignore = true
coding_scope = ["os", "web", "agent", "skill", "bot"]  # default tool scope

[toolset.os.shell]
timeout_secs = 120
output_byte_limit = 65536

[shortcuts]
send = ["Enter"]
newline = ["Shift+Enter", "Alt+Enter"]
quit = ["Ctrl+D", "Ctrl+Q"]
confirm_quit = true

[leader]
enabled = false                       # Phase 5
socket = "~/.nebo/leader.sock"
use_leader = false                    # TUI attaches when true

[permissions]
default_mode = "default"              # permission-mode enum

[plugins]
paths = []
disabled = []

[mcp]
# merged from settings; project .mcp.json overrides

[hooks]
# Phase 4 — see Appendix F
```

Env overrides: `NEBO_CODE_HOME`, `NEBO_CODE_RESPECT_GITIGNORE`, `NEBO_CODE_LEADER=1`.

---

## Security & Privacy

| Concern | Mitigation |
|---------|------------|
| Arbitrary shell execution | Default approval gate; `SECURITY.md` safeguard in `Registry::execute()` always runs |
| Path escape | `allowed_paths` = canonicalized `cwd` (+ worktree root); deny `..` writes outside |
| Secret exfiltration | `tools/memory_guard` on memory writes; existing redaction pipeline |
| `--always-approve` / bypass | Audit log at `~/.nebo/logs/code-audit.jsonl`; warn on launch |
| Hooks | Require `/hooks-trust` / workspace trust file before executing project hooks |
| MCP servers | Same trust model as desktop MCP integrations |
| Session files | Local only under `~/.nebo/sessions/`; no upload unless `trace --upload` opted in |
| API keys | Existing keyring + `settings.json` encryption; never print in `--debug` |

Headless in CI: recommend `--permission-mode dontAsk` + explicit `--allow` rules, not `--always-approve`.

---

## Testing Strategy

| Layer | Approach |
|-------|----------|
| Headless contract | Golden tests: `nebo code -p` with mock provider → compare `events.jsonl` to fixtures in `tests/fixtures/code-cli/` |
| Grok parity | Record `grok -p --output-format streaming-json` on canonical tasks; diff event shapes |
| Session I/O | Round-trip `summary.json` + `events.jsonl`; resume with `-c` |
| Permissions | Unit tests for `--allow`/`--deny` glob engine |
| Driver | Integration test: `CodeSessionDriver` → real `Runner` with temp dir |
| TUI | Manual QA checklist (Appendix A); optional `ratatui` snapshot tests Phase 5 |
| ACP | Conformance tests against `agent-client-protocol` test vectors |
| Regression | Add one fixture per closed GitHub issue |

CI gate: Phase 0 merges require headless fixture pass; Phase 2 requires TUI checklist sign-off.

---

## GA Criteria

### Beta (Phase 1 complete)

- [ ] `nebo code -p` passes 5 golden fixtures  
- [ ] `-c` / `-r` resume works across invocations  
- [ ] `inspect --json` stable schema  
- [ ] `models`, `login`, `sessions list` functional  
- [ ] `nebo chat` prints deprecation notice  
- [ ] Docs: quick start in README  

### GA (Phase 3 complete — "Grok-competitive")

- [ ] Full TUI (Phase 2) + ACP `agent stdio`  
- [ ] All **P0–P2** flags and subcommands in Appendix E implemented or explicitly N/A  
- [ ] Permission modes: default, acceptEdits, bypassPermissions, plan  
- [ ] @file picker + diff approve/reject  
- [ ] Session export Markdown  
- [ ] Worktree create + list  
- [ ] Zed external-agent doc tested  
- [ ] Security audit: allowed_paths, audit log, no key leakage  

### Full parity (Phase 5 — optional)

- [ ] Leader process  
- [ ] `--best-of-n`, `--minimal`, `import`, `wrap`  
- [ ] Hooks parity with Grok  
- [ ] `nebocode` alias shipped  

---

## Decisions (resolved)

| # | Question | **Decision** | Rationale |
|---|----------|--------------|-----------|
| 1 | Binary alias | **Phase 5:** optional `nebocode` → `nebo code` symlink in package | Avoid top-level confusion with `nebo` desktop |
| 2 | Leader vs serve | **Phase 5a:** `nebo serve` is the leader when running; **Phase 5b:** dedicated `agent leader` socket | Reuse existing server first |
| 3 | Session DB mirror | **Filesystem canonical**; optional SQLite index table Phase 2 for desktop "Recent terminal sessions" | Simpler, matches Grok |
| 4 | Default agent | **Per-repo:** `AGENTS.md` > `.nebo/agents/` > global companion; `--agent` overrides | Matches Grok agent profiles |
| 5 | `--restore-code` | **Git commit in `summary.json` per turn**; restore checks out `git_head` on resume | Simpler than full snapshot |
| 6 | `agent serve` | **Document** `nebo serve` + `/ws` + port 27895; no separate WS server | Avoid duplicate servers |
| 7 | Windows | **Headless all platforms Phase 0**; TUI Phase 2 macOS/Linux; Windows TUI Phase 4 | ratatui risk |
| 8 | OAuth / login | **Provider keys via existing settings** + browser OAuth for NeboAI marketplace; `login --device` Phase 3 | No xAI OAuth |
| 9 | `Origin` for CLI | **`Origin::User`** (not new variant) | Already means "CLI + web UI" in `origin.rs` |
| 10 | Output flag name | **`--output-format`** only (drop `--format`) | Match Grok exactly |
| 11 | Memory default | **On** without flag; `--no-memory` disables | Nebo differentiator |
| 12 | `nebo mcp serve` | **Keep** as inverse MCP bridge; document vs `nebo code` | Different use case |

---

## Appendix A: Grok Experience Observation Checklist

Use during Phase 2+ QA — RE *via behavior* (`grok --help`, `grok -p --output-format json`, session files).

### CLI surface
- [ ] Every P0 flag in parity matrix has `nebo code --help` entry  
- [ ] `nebo code --help` groups flags like Grok (session, tools, permission, worktree)  
- [ ] Subcommands discoverable from top-level help  

### TUI
- [ ] First launch: auth, model pick, no server setup  
- [ ] Turn counter and token usage visible  
- [ ] Tool card expands with live stdout  
- [ ] Diff shown before/after write with reject path  
- [ ] `Ctrl+C` cancels in-flight tool, not whole session  
- [ ] `@` picker respects gitignore; `!` exposes dotfiles  
- [ ] `--no-alt-screen` usable in tmux over SSH  
- [ ] Subagent panel shows parallel tasks  

### Headless
- [ ] `streaming-json` events parse with `jq`  
- [ ] `-c` / `-r` resume mid-task  
- [ ] `--permission-mode acceptEdits` auto-approves writes only  
- [ ] `--fork-session` branches without losing parent  

### Subcommands
- [ ] `inspect --json` lists skills, MCP, agents, hooks  
- [ ] `sessions search` finds by prompt text  
- [ ] `export` produces readable Markdown  
- [ ] `worktree list` tracks isolated branches  

Capture recordings + JSON logs under `tests/fixtures/code-cli/`.

---

## Appendix B: Key Files to Touch

| File | Change |
|------|--------|
| `crates/cli/src/main.rs` | Add `Code` and `Agent` subcommands |
| `crates/cli/src/code/` | **New** — driver, headless, TUI, inspect |
| `crates/cli/src/acp/` | **New** — ACP stdio server |
| `crates/agent/src/runner.rs` | Ensure `Origin::Cli` + plan approval events work in non-WS sink |
| `crates/server/src/chat_dispatch.rs` | Extract shared `run_chat_events` consumer for attach mode |
| `crates/tools/src/registry.rs` | `coding` tool scope definition |
| `crates/config/src/` | `code.toml` loading |
| `docs/sme/CHAT_SYSTEM.md` | Document `acp:` sessions + CLI channel |
| `README.md` | Quick start for `nebo code` |

---

## Appendix C: Related Work

- `docs/prd/agent-filesystem-watcher.md` — agents appear live; terminal can target them  
- `docs/sme/CHAT_SYSTEM.md` — session keys, `acp:` prefix  
- `docs/sme/TOOLS_SME.md` — STRAP tool inventory  
- `docs/sme/TESTING_SME.md` — `nebo test run` harness (prototype driver)  
- `crates/cli/src/mcp_serve.rs` — inverse MCP pattern (stdio bridge)  

---

## Appendix D: Grok top-level `--help` skeleton (v0.2.87)

See § Grok CLI Surface Inventory for annotated parity. Run `grok --help` for full descriptions.

---

## Appendix E: Nested subcommand specifications

Per-command spec for engineering. **Nebo path** = `nebo code <path>` unless noted.

### `agent` — `nebo code agent`

| Subcmd | Grok flags | Nebo implementation | Phase |
|--------|------------|----------------------|-------|
| `stdio` | `--leader-socket` | ACP JSON-RPC on stdin/stdout; `agent-client-protocol` | 3 |
| `headless` | `--grok-ws-url`, `--grok-ws-origin` | WS client → `nebo serve` `/ws` (no xAI relay) | 3 |
| `serve` | `--bind` (default `127.0.0.1:2419`), `--secret`, `--remote` | **N/A** — document `nebo serve :27895` | — |
| `leader` | `--no-exit-on-disconnect`, `--relay-on-demand`, `--no-auto-update` | Unix socket `~/.nebo/leader.sock`; or alias to `nebo serve` | 5 |

**Parent flags:** `--reauth`, `-m`, `--reasoning-effort`, `--always-approve`, `--agent-profile`, `--plugin-dir` (repeatable), `--leader`, `--no-leader`, `--debug`, `--debug-file`.

### `sessions` — `nebo code sessions`

| Subcmd | Args / flags | Nebo implementation | Phase |
|--------|--------------|----------------------|-------|
| `list` | (none) | List `~/.nebo/sessions/<cwd>/` summaries | 1 |
| `search` | `<QUERY>`, `-n/--limit` (default 20) | Search `summary.json` title + first prompt | 2 |
| `delete` | `<SESSION_ID>` | Remove session directory | 2 |

### `mcp` — `nebo code mcp`

| Subcmd | Args / flags | Nebo implementation | Phase |
|--------|--------------|----------------------|-------|
| `list` | | Merge settings + `.mcp.json` + project `.nebo/code.toml` | 2 |
| `add` | `<NAME> [CMD\|URL] [ARGS...]`, `-t stdio\|http\|sse`, `-s user\|project`, `-e`, `-H` | Write to config; same UX as Grok examples | 2 |
| `remove` | `<NAME>` | | 2 |
| `doctor` | | Probe each MCP server connectivity | 2 |

### `memory` — `nebo code memory`

| Subcmd | Grok | Nebo | Phase |
|--------|------|------|-------|
| `clear` | Clear workspace memory files | Clear workspace-scoped memory entities in DB + files | 2 |
| `list` | — | **Nebo extra:** list memory keys for cwd | 2 |
| `search` | — | **Nebo extra:** hybrid search via embedding | 3 |

### `plugin` — `nebo code plugin`

| Subcmd | Grok | Nebo mapping | Phase |
|--------|------|--------------|-------|
| `list` | Installed plugins | `napp` registry + marketplace installed | 3 |
| `install` | git URL or local path | Install code / marketplace slug | 3 |
| `uninstall` | alias `rm`, `remove` | | 3 |
| `update` | Update plugin(s) | Artifact update system | 3 |
| `enable` / `disable` | | Plugin enabled flag in DB | 3 |
| `details` | Component inventory | | 4 |
| `validate` | Manifest validation | `napp` signing/reader | 4 |
| `tag` | Release git tag from manifest | **Defer** — publisher tooling | — |
| `marketplace list/add/remove/update` | Marketplace sources | NeboAI marketplace sources | 3 |

### `leader` — `nebo code leader`

| Subcmd | Flags | Nebo | Phase |
|--------|-------|------|-------|
| `list` | `--json` | List PIDs / sockets holding leader | 5 |
| `info` | | Leader uptime, clients, session count | 5 |
| `kill` | | Stop all leaders | 5 |
| `profile` | | CPU profiling (optional) | 5 |

### `worktree` — `nebo code worktree`

| Subcmd | Args | Nebo | Phase |
|--------|------|------|-------|
| `list` | | Git worktrees created by `nebo code -w` | 3 |
| `show` | `<ID_OR_PATH>` | Metadata: branch, session ids, cwd | 3 |
| `rm` | | Remove worktree + tracking entry | 3 |
| `gc` | | Prune stale worktrees | 3 |
| `db` | | SQLite/metadata maintenance | 4 |

### `login` / `logout`

| Cmd | Grok flags | Nebo | Phase |
|-----|------------|------|-------|
| `login` | `--oauth`, `--device-auth` | Browser provider setup + NeboAI OAuth; device code Phase 3 | 1 |
| `logout` | | Clear `settings.json` provider tokens | 1 |

### `export` / `import` / `trace`

| Cmd | Grok flags | Nebo | Phase |
|-----|------------|------|-------|
| `export <ID> [OUT]` | `-c/--clipboard` | Markdown transcript from `events.jsonl` | 2 |
| `import [TARGETS...]` | `--list`, `--json` | Import Grok `.jsonl` / Nebo sessions | 5 |
| `trace <ID>` | `--local`, `-o`, `--json` | Tar.gz of session dir + traces; no remote upload default | 4 |

### `inspect`

| Flags | Nebo output sections | Phase |
|-------|---------------------|-------|
| `--json` | `project_rules`, `skills`, `plugins`, `mcp_servers`, `agents`, `hooks`, `tools_effective`, `model`, `cwd`, `config_sources[]` | 1 |

### `models`

No args. List providers + models from `config/models.rs` + active auth profiles; exit 0.

### `setup`

Fetch managed config / first-run wizard. Extends `nebo onboard` with provider prompt + `code.toml` defaults.

### `completions <SHELL>`

`bash | elvish | fish | powershell | zsh` — clap-generated.

### `dashboard`

Grok: agent-native session overview TUI. Nebo Phase 5: terminal dashboard or deep-link `http://localhost:27895/activity`.

### `wrap <CMD>...`

OSC 52 clipboard bridge for SSH/containers. Phase 5.

### `update` / `version`

Map to existing top-level `nebo` updater — not nested under `code`.

---

## Appendix F: Hooks system (Phase 4)

Grok-compatible lifecycle hooks. Config: `~/.nebo/code.toml` `[hooks]` or `.nebo/hooks/*.json`.

### Hook events (parity)

| Event | When | Can block? |
|-------|------|------------|
| `PreToolUse` | Before tool executes | Yes (exit 2) |
| `PostToolUse` | After success | No |
| `PostToolUseFailure` | After failure | No |
| `UserPromptSubmit` | User sent prompt | Yes |
| `SessionStart` | Session created | No |
| `SessionEnd` | Session closed | No |
| `Stop` | Agent finished turn | No |
| `SubagentStart` / `SubagentStop` | Child agent lifecycle | No |
| `PreCompact` / `PostCompact` | Compaction | No |
| `Notification` | System notify | No |
| `InstructionsLoaded` | AGENTS.md merged | No |
| `CwdChanged` | cwd changed | No |

### Hook command protocol

- Stdin: JSON event payload  
- Stdout: optional JSON `{"decision":"block","reason":"..."}`  
- Exit `0` = ok, `2` = block, other = non-blocking error  

Trust: `.nebo/hooks-trust.json` per repo; `/hooks-trust` in TUI.

---

## Appendix G: ACP method map (Phase 3)

Implement subset required by Zed external agents ([ACP spec](https://agentclientprotocol.com)):

| Method | Purpose |
|--------|---------|
| `initialize` | Handshake, capabilities |
| `session/new` | `cwd`, return `sessionId` |
| `session/load` | Resume by id |
| `session/prompt` | User turn → stream agent events |
| `session/cancel` | Cancel in-flight turn |
| `tool/list` | Optional — expose STRAP tools |

Session key: `acp:<sessionId>`. Wire through `CodeSessionDriver` same as TUI.

---

## Appendix H: Tool ID alias table (headless `--tools`)

| Grok internal ID | Display | Nebo STRAP |
|------------------|---------|------------|
| `read_file` | read | `os` action `read` |
| `search_replace` | edit | `os` action `write`/`edit` |
| `grep` | grep | `os` action `grep` |
| `list_dir` | list | `os` action `list` |
| `run_terminal_cmd` | bash | `os` action `shell` |
| `web_search` | web search | `web` action `search` |
| `web_fetch` | fetch | `web` action `fetch` |
| `task` | subagent | `agent` action `spawn` |
| `todo_write` | todos | **Defer** — TUI task panel local state |
| `memory_search` | memory | `bot` action `search` |
| `Agent` / `Agent(type)` | subagent deny | `agent` spawn policy |

CLI accepts **either** Grok aliases or STRAP names for compatibility.

---

## Appendix I: Migration & coexistence

| Existing cmd | After Nebo Code |
|--------------|-----------------|
| `nebo chat` | Prints: "Use `nebo code -p` or `nebo code` for agentic chat." Keeps raw provider mode `--raw` flag if needed |
| `nebo mcp serve` | Unchanged — Nebo as MCP *tool server* for Cursor/Claude Desktop |
| `nebo test run` | Unchanged — internal harness; shares driver code with `nebo code` |
| `nebo serve` | Recommended leader backend (Phase 5a) |
| Desktop chat | Same DB memories; terminal sessions in `~/.nebo/sessions/` until DB mirror ships |

---

## Summary

Nebo Code is not a greenfield agent — it is a **terminal experience layer** on the existing companion brain. The **Grok CLI surface inventory** (§ above) is the contract: ~45 flags, 20 subcommands, 5 agent modes. We implement against that checklist, not against vibes.

Nebo wins where Grok cannot: multi-provider, memory on by default, marketplace skills/plugins, desktop continuity, VM sandbox.

**Ship order:** Phase 0 headless flags → Phase 1 inspect/sessions/login → Phase 2 TUI → Phase 3 ACP/worktrees → Phase 4 advanced → Phase 5 leader/best-of-n.

**PRD completeness (v2):** Top-level flags, nested subcommands (Appendix E), schemas, config, hooks, ACP map, GA gates, and resolved decisions. Remaining deferrals are explicit (Phase 5 / N/A rows).

**Gap vs Grok GA:** ~40% of surface ships in Phase 3+ (worktrees, ACP, plugins, leader). Phase 0–1 is deliberately useful without full parity.