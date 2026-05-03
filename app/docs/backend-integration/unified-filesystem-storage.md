# Backend Architecture Change: Unified Filesystem Storage

Generated 2026-05-01.

## Problem

Artifact storage is split across two systems:

| Artifact | Current Storage | Location |
|----------|----------------|----------|
| Plugins | Filesystem | `~/.nebo/nebo/plugins/<slug>/<version>/` |
| Skills | Filesystem | `~/.nebo/nebo/skills/<slug>/<version>/` (checked via `is_skill_locally_installed()`) |
| Agents | **SQLite** | `store.agent_installed_by_name()` — rows in database |

This creates inconsistency in install, uninstall, list, and resolve code paths. Plugins and skills have a clear filesystem layout with versioned directories and manifest files, while agents are rows in a table with no file-based representation.

## Proposed: All Artifacts on Filesystem

Every installed artifact should follow the same filesystem pattern. One directory per artifact type, one subdirectory per slug, one subdirectory per version.

```
~/.nebo/
├── nebo/                    # marketplace installs
│   ├── plugins/<slug>/<version>/
│   │   ├── plugin.json      # manifest
│   │   └── <binary>         # platform binary
│   ├── skills/<slug>/<version>/
│   │   ├── skill.json       # manifest
│   │   └── SKILL.md         # skill instructions
│   └── agents/<slug>/<version>/
│       ├── agent.json       # manifest (name, description, model, dependencies)
│       └── AGENT.md         # agent persona / instructions
├── user/                    # user-created (local overrides)
│   ├── plugins/<slug>/<version>/
│   ├── skills/<slug>/<version>/
│   └── agents/<slug>/<version>/
└── bundled/                 # shipped with app (read-only)
    ├── skills/
    └── agents/
```

## Unified Operations

All three artifact types share the same operations with the same filesystem semantics:

### Install
1. Download `.napp` archive from store
2. Verify ED25519 signature
3. Extract to `~/.nebo/nebo/<type>/<slug>/<version>/`
4. Write manifest (`plugin.json` / `skill.json` / `agent.json`)

### Uninstall
1. Remove `~/.nebo/nebo/<type>/<slug>/` directory

### List Installed
1. Scan `~/.nebo/nebo/<type>/` for subdirectories
2. Read each `<type>.json` manifest
3. Merge with `~/.nebo/user/<type>/` and `~/.nebo/bundled/<type>/` (user overrides bundled)

### Resolve (find artifact by name)
1. Check `~/.nebo/user/<type>/<slug>/` first (user override)
2. Check `~/.nebo/nebo/<type>/<slug>/` (marketplace install)
3. Check `~/.nebo/bundled/<type>/<slug>/` (shipped default)
4. Return highest version found at first matching tier

### Is Installed
1. Check if `~/.nebo/nebo/<type>/<slug>/` directory exists (any version)

## Agent Manifest Shape (`agent.json`)

```json
{
  "id": "uuid",
  "name": "Research Assistant",
  "slug": "research-assistant",
  "version": "1.2.0",
  "description": "Deep research agent with web search capabilities",
  "author": "@nebo/agents",
  "source": "marketplace",
  "is_enabled": true,
  "model": "claude-sonnet-4-6",
  "installed_at": "2026-05-01T12:00:00Z",
  "dependencies": {
    "skills": ["@nebo/skills/web-search", "@nebo/skills/summarize"],
    "plugins": ["@nebo/plugins/browser"]
  }
}
```

## Migration Path

### Phase 1: Add filesystem write on agent install
- When `install_store_product()` installs an agent, also write to `~/.nebo/nebo/agents/<slug>/<version>/`
- Keep SQLite write for backwards compatibility

### Phase 2: Read from filesystem first
- `list_installed_agents()` scans filesystem, falls back to SQLite
- `is_agent_installed()` checks filesystem first

### Phase 3: Remove SQLite agent storage
- Drop `installed_agents` table (or equivalent)
- All reads/writes go through filesystem
- Single code path for all artifact types

## Benefits

1. **One code path** — install/uninstall/list/resolve logic is shared across all artifact types
2. **Inspectable** — users can `ls ~/.nebo/nebo/agents/` to see what's installed
3. **Portable** — copy the directory to move installs between machines
4. **Versioned** — multiple versions can coexist, rollback is trivial
5. **Offline-friendly** — no database lock contention, works with file sync
6. **Consistent** — no more "is this type in DB or filesystem?" questions

## Files to Change (Backend)

| File | Change |
|------|--------|
| `crates/server/src/handlers/store.rs` | Write agent manifest to filesystem on install |
| `crates/napp/src/lib.rs` (or new `agent.rs`) | Add `AgentStore` mirroring `PluginStore` pattern |
| `crates/napp/src/plugin.rs` | Extract shared filesystem logic into trait or common module |
| `crates/config/src/defaults.rs` | Add `agents_dir()` alongside `plugins_dir()` |
| `crates/store/src/lib.rs` | Deprecate `agent_installed_by_name()` DB queries |

## Frontend Impact

None. The frontend calls `installStoreProduct(id)` / `uninstallStoreProduct(id)` and `listAgents()`. The storage backend is invisible to the frontend — this is a purely backend change.
