# PRD: Agent Filesystem Watcher — Live Sync

## Problem

Agents dropped into `user/agents/` or `nebo/agents/` don't appear in the sidebar until Nebo restarts. The filesystem watcher exists (`agent_loader.watch()`) but only updates an in-memory cache — it never syncs to the DB or notifies the frontend.

Additionally, some agents exist only in the DB (created via API/marketplace) with no filesystem backing (`nappPath: null`). The system has two competing sources of truth.

## Current Behavior

1. `agent_loader.watch()` detects Create/Modify/Remove of AGENT.md, agent.json, manifest.json, .napp
2. Re-scans both directories, updates `agents: Arc<RwLock<HashMap>>` in memory
3. **Stops there.** No DB sync, no WS broadcast, no activation.

Result: new agent directory → invisible until restart.

## Desired Behavior

1. Watcher detects new/changed/removed agent files
2. Diffs against current DB state
3. **New agent on filesystem:** creates DB record (`is_enabled=1`), inserts into `agent_registry`, broadcasts `agent_activated` WS event → sidebar updates instantly
4. **Changed agent on filesystem:** updates DB record (agent_md, frontmatter, description), updates `agent_registry`, broadcasts `agent_updated` WS event
5. **Removed agent from filesystem:** sets `is_enabled=0` in DB, removes from `agent_registry`, broadcasts `agent_deactivated` WS event → disappears from sidebar

## Architecture

### Option A: Watcher gets AppState access (recommended)

The watcher currently runs in the `napp` crate which has no access to `AppState` (store, hub, registry). Fix: move the watch loop into the `server` crate, or pass a callback/channel from server into the watcher.

```
agent_loader.watch() → channel → server receiver loop
  → store.create_agent() / store.update_agent()
  → agent_registry.insert() / .remove()
  → hub.broadcast("agent_activated" / "agent_updated" / "agent_deactivated")
```

### Option B: Event channel from napp → server

`watch()` returns a `tokio::sync::mpsc::Receiver<AgentFsEvent>` with variants:
- `AgentAdded(LoadedAgent)`
- `AgentChanged(LoadedAgent)`
- `AgentRemoved(String)` (name)

Server subscribes to this channel at startup and handles DB + registry + WS.

## Scope

### Must Have
- New agent directory → auto-create in DB + activate + WS broadcast
- Changed AGENT.md/agent.json → update DB + WS broadcast
- Removed agent directory → deactivate + WS broadcast
- Frontend handles `agent_activated`, `agent_updated`, `agent_deactivated` WS events to update sidebar in real-time

### Should Have
- Diff logic: only update DB if content actually changed (avoid spurious writes on editor save-swap files)
- Handle the "AGENT.md is the marker" convention: directory without AGENT.md = not an agent

### Out of Scope (separate work)
- Reconciling DB-only agents with no filesystem backing (the historical data issue)
- Marketplace .napp extraction pipeline
- Agent hot-reload of running sessions (active chats keep their current prompt until next message)

## Key Files

| File | What to change |
|------|---------------|
| `crates/napp/src/agent_loader.rs` | Make `watch()` emit events instead of silently updating cache |
| `crates/server/src/lib.rs` | Subscribe to watcher events, handle DB sync + registry + WS |
| `crates/server/src/handlers/agents.rs` | Reuse `activate_agent` / `deactivate_agent` logic from existing endpoints |
| `crates/db/src/queries/agents.rs` | No changes needed — create/update/delete already exist |
| `app/src/lib/stores/` or WS handler | Ensure frontend sidebar reacts to `agent_activated`/`agent_deactivated` events |

## Frontend WS Events

The frontend likely already handles these events for marketplace installs. Verify:
- `agent_activated` → add to sidebar agent list
- `agent_deactivated` → remove from sidebar agent list
- `agent_updated` → refresh agent name/description in sidebar

## Testing

1. Start Nebo, confirm existing agents load
2. Drop a new agent directory into `user/agents/` with AGENT.md → should appear in sidebar within ~2s
3. Edit the AGENT.md → sidebar should reflect updated name/description
4. Delete the agent directory → should disappear from sidebar
5. Rename AGENT.md to something else → agent should deactivate (no marker file)
