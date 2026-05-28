# Plugin System Parity — Status Tracker

## Purpose

Tracks Nebo's progress toward plugin capability parity with OpenClaw and Hermes. The full implementation plan lives in the Claude Code plans directory. Detailed type/API documentation lives in `PLUGIN_SYSTEM.md` §4 and §23.

## Architecture Decision (Settled)

**Manifest-driven capabilities, process-boundary execution, existing infrastructure reuse.**

- Plugins stay **out-of-process** — subprocess execution via stdin/stdout JSON/NDJSON
- `.napp` remains the universal distribution envelope (binary + manifest + skills + assets)
- All capabilities declared in `plugin.json` — no runtime capability negotiation
- The generic `plugin` tool stays as fallback; structured typed tools are added alongside it
- HookDispatcher, tool Registry, Provider trait, plugin_settings table — all reused, not rebuilt

This is stronger than OpenClaw (in-process TypeScript) and Hermes (in-process Python) for marketplace trust, while matching their extensibility surface.

## Implementation Status

### Done

| Phase | What | Key Changes |
|-------|------|-------------|
| 0A | Fix "latest" version install | `install_from_napp_inner` resolves "latest" to manifest semver before creating directories |
| 0B | Manifest validation | `PluginManifest::validate()` — slug format, semver, binary name safety, auth/event consistency |
| 0D | Plugin-to-plugin dependencies | `dependencies[]` in plugin.json; cascade resolves recursively; dep env vars injected |
| 1A | Structured capabilities manifest | `capabilities` field with `tools`, `hooks`, `commands`, `routes`, `providers` sub-types |
| 1C | Generated typed tools | Replaced by STRAP `PluginTool` with consolidated action routing |

### Not Started

| Phase | What | Blocking? |
|-------|------|-----------|
| 0C | Installed plugin index (SQLite) | No — extends existing `plugin_registry` table |
| 2 | Plugin hook bridge | No — `PluginHookCaller` implementing `HookCaller` trait, out-of-process |
| 3 | Config schema & settings | No — reuses existing `plugin_settings` table + JSON Schema validation |
| 4 | Permissions manifest | No — env filtering, timeout caps, install-time approval |
| 5 | Plugin commands | No — extend `chat_dispatch.rs` slash command matching |
| 6 | Plugin providers (sidecar) | No — `PluginProvider` implementing `ai::Provider`, NDJSON streaming |
| 7 | Plugin HTTP routes | No — catch-all proxy at `/plugins/{slug}/api/*path` |
| 8 | Discovery diagnostics | No — structured error collection and reporting |

### Out of Scope

| Capability | Why |
|------------|-----|
| Channel plugins | Communication locked to NeboAI SDK in `crates/comm/` |
| Dashboard/UI plugins | Requires frontend plugin loader, CSP, bridge API — separate project |
| In-process plugin loading | Violates out-of-process trust boundary |
| Memory/context plugins | Memory/prompt assembly has no plugin surface |
| Reusable services | No service registration model needed yet |

## Comparison Reference

How Nebo compares to OpenClaw and Hermes after Phase 0+1 work:

| Dimension | Nebo (Current) | OpenClaw | Hermes |
|-----------|---------------|----------|--------|
| Execution model | Out-of-process subprocess | In-process TypeScript | In-process Python |
| Trust boundary | Signed `.napp` + SHA256 + ED25519 | npm package (no signing) | pip package (no signing) |
| Tool registration | Generic tool + manifest-declared typed tools | Per-plugin typed tools with schemas | Per-plugin tools in global registry |
| Hook system | 12 hooks (manifest ready, bridge pending) | 29 hooks (all plugin-accessible) | 13 hooks (all plugin-accessible) |
| Provider registration | Manifest ready, impl pending | 10+ types (LLM, TTS, voice, image, etc.) | None |
| Channel registration | Not possible (NeboAI SDK) | 20+ adapters | None |
| Config schemas | Planned (Phase 3) | Zod + UI hints + JSON Schema | plugin.yaml + config.yaml |
| CLI commands | Manifest ready, dispatch pending | Top-level with lazy descriptors | Slash + CLI commands |
| Plugin-to-plugin deps | Implemented | None (npm handles deps) | None |
| Manifest validation | Implemented (16 tests) | None | Minimal |
| Marketplace distribution | Signed `.napp` with install codes | npm publish | pip/directory |

**Nebo's advantages:** Binary verification, marketplace trust model, process isolation, plugin-to-plugin deps.
**Remaining gaps:** Hook runtime bridge, provider sidecar, config schemas, permissions enforcement.

## Key Files

| File | What |
|------|------|
| `crates/napp/src/plugin.rs` | Core types, PluginStore, validation, capabilities, dependencies |
| `crates/tools/src/plugin_tool.rs` | PluginTool (STRAP domain tool), run_plugin_command |
| `crates/server/src/deps.rs` | Dependency cascade (now supports plugin→plugin) |
| `crates/server/src/codes.rs` | PLUG code handling + tool registration |
| `docs/sme/PLUGIN_SYSTEM.md` | Full SME reference (§4 types, §11 cascade, §23 boundaries) |
| `docs/publishers-guide/plugins.md` | Publisher-facing documentation |
