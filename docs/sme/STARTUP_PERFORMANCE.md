# Startup Performance

**Status:** Documented, not yet fixed
**Severity:** High — 82s to first HTTP response
**Observed:** 2026-05-12, `cargo tauri dev` on macOS (137 plugins, 2559 skills, 10 agents)

## Problem

Nebo takes ~82 seconds from process start to accepting its first HTTP connection. The frontend polls `/health` every 2 seconds and gets `ECONNREFUSED` the entire time. Tauri gives up at 15s and launches the window anyway, showing a broken UI until the server comes up.

Two blocking operations run on the critical path **before** the server binds its port:

| Bottleneck | Duration | Code Path |
|------------|----------|-----------|
| Skill loading | ~20s | `lib.rs:625` → `SkillLoader::load_all()` |
| Auth cache | ~61s | `lib.rs:1213` → `PluginStore::refresh_auth_cache()` |

## Startup Timeline

Annotated from actual logs (`cargo tauri dev`, 2026-05-12):

```
T+0.0s   server thread starts
T+0.3s   DB migration complete (fast — already up to date)
T+0.5s   CLI detection, AI providers loaded
T+0.5s   ─── skill_loader.load_all().await BEGINS ───
         ... recursive directory walks, YAML parsing, dependency verification ...
         WARN skill degraded: unmet dependencies (trickles in over ~5s)
T+15s    "Server did not become ready in 15s, launching window anyway"
T+20.7s  "loaded skills count=2559" ← skill loading complete
T+20.7s  ─── skill_loader.load_all().await ENDS ───
T+20.7s  advisors loaded (count=5), sandbox init, napp registry, agent loader
T+21.0s  ─── plugin_store.refresh_auth_cache().await BEGINS ───
         ... spawning 137 plugin binaries with auth-status ...
         /health polls get ECONNREFUSED every 2s for the next minute
T+81.7s  "auth cache populated: 137 plugins checked"
T+81.7s  ─── plugin_store.refresh_auth_cache().await ENDS ───
T+81.7s  agent workers start (3 agents)
T+82.0s  routes built, TcpListener::bind(), axum::serve()
T+82.9s  first /health succeeds, WS connects
```

## Server Startup Structure

The critical path in `start_server()` (`crates/server/src/lib.rs`):

```
line  407   TcpListener::bind (sync check — validates port is free, drops immediately)
line  625   skill_loader.load_all().await           ← BLOCKS ~20s
line  626   skill_loader.watch()
...         (advisors, sandbox, registry, agents — fast)
line 1213   plugin_store.refresh_auth_cache().await  ← BLOCKS ~61s
line 1217   agent worker startup (needs auth cache)
...         (routes, middleware, AppState)
line 1739   tokio::net::TcpListener::bind            ← ACTUAL server bind
line 1743   axum::serve(listener, app)               ← Start accepting connections
```

Everything between line 407 and 1739 must complete before the server accepts any connection.

## Bottleneck 1: Skill Loading (~20 seconds)

**Entry:** `skill_loader.load_all().await` at `lib.rs:625`
**Implementation:** `SkillLoader::load_all()` at `skills/loader.rs:82-202`

### What happens

6 sequential stages:

1. **Bundled skills** — in-memory `BUNDLED_SKILLS` map, parsed from embedded bytes. Fast.
2. **Installed skills** — `load_skills_from_nested_dir(&installed_dir)` → `walk_for_marker("SKILL.md")` recursive walk. **This is the slow one.**
3. **Sealed .napp skills** — `load_sealed_skills()` decrypts + verifies each `.napp` archive.
4. **Plugin-embedded skills (marketplace)** — for each plugin in `nebo/plugins/*/`, calls `load_skills_from_nested_dir()` again. 50+ separate walks.
5. **Plugin-embedded skills (user)** — same for `user/plugins/*/`.
6. **User skills** — flat scan of `user/skills/`.
7. **Dependency verification** — `verify_dependencies()` loops through all 2559 skills.

### Why it's slow

- **Double `read_dir()` per directory.** `walk_for_marker()` (`napp/reader.rs:240-274`) lists directory entries, then for each subdirectory calls `has_marker()` which does **another** `read_dir()` on the same directory to check for `SKILL.md`. That's 2× syscalls for every directory in the tree.

- **Sequential I/O.** All 2559 SKILL.md files are read from disk + YAML-parsed one at a time. No parallelism.

- **Redundant nested walks.** Plugin-embedded skills (stages 4-5) trigger 50+ separate recursive walks — one per plugin directory.

- **Serial dependency verification.** `verify_dependencies()` iterates all 2559 skills, checking each one's plugin dependencies against the plugin store.

### Key files

| File | Function | Issue |
|------|----------|-------|
| `crates/tools/src/skills/loader.rs:82-202` | `load_all()` | 6 sequential stages |
| `crates/tools/src/skills/loader.rs:701-738` | `load_skills_from_nested_dir()` | Reads each SKILL.md from disk |
| `crates/napp/src/reader.rs:240-274` | `walk_for_marker()` | Double read_dir per directory |
| `crates/napp/src/reader.rs:264-274` | `has_marker()` | Redundant read_dir to check marker |
| `crates/tools/src/skills/loader.rs:932-1008` | `verify_dependencies()` | Serial 2559-item loop |

## Bottleneck 2: Auth Cache (~61 seconds)

**Entry:** `plugin_store.refresh_auth_cache().await` at `lib.rs:1213`
**Implementation:** `PluginStore::refresh_auth_cache()` at `plugin.rs:529-571`

### What happens

1. Lists all 137 installed plugins
2. Filters to plugins that have `auth.commands.status` defined
3. For each, spawns the plugin binary with the `auth-status` command via `tokio::process::Command`
4. Collects results with `futures::future::join_all()` (parallel)
5. Stores results in `auth_cache: RwLock<HashMap<String, bool>>`

### Why it's slow

- **Subprocess spawn saturation.** Even with `join_all()`, spawning 30-50+ concurrent processes saturates the OS scheduler. Each binary takes 0.5-2s including keychain access on macOS.

- **No per-process timeout.** A hung plugin binary blocks the entire `join_all()` forever. There's no timeout on `run_auth_status_check()` (`plugin.rs:1809-1827`).

- **Keychain serialization on macOS.** The OS keychain dialog serializes concurrent access, so parallel launches don't actually parallelize keychain reads.

- **History.** This was previously a background task (non-blocking). We moved it to `await` to fix a bug where 6 concurrent keychain prompts appeared during startup. The fix was correct — keychain prompts are gone — but it created this 61-second blocking bottleneck.

### Key files

| File | Function | Issue |
|------|----------|-------|
| `crates/napp/src/plugin.rs:529-571` | `refresh_auth_cache()` | join_all on 137 binaries |
| `crates/napp/src/plugin.rs:1809-1827` | `run_auth_status_check()` | No timeout, spawns subprocess |

## Design Constraints

Any solution must respect these dependencies:

1. **Auth cache → agent watch processes.** Watch processes (e.g., GWS `gmail +watch`) access the keychain. If they run concurrently with `auth-status` checks, the user gets prompted 6+ times. Auth cache must complete before watches spawn.

2. **Skills → tool registry.** Skills define available tools. The tool registry is built from loaded skills. Chat/agent operations need the registry to be populated.

3. **Agent configs → skills.** `validate_agent_dependencies()` checks that agents' required skills are loaded and not degraded.

4. **Workflow triggers → agent configs.** Schedule/watch/heartbeat triggers are registered during agent worker startup, which reads agent configs.

5. **Frontend → `/health` + `/ws`.** The Tauri wrapper and Vite dev proxy poll `/health` to know when the server is ready. The UI connects via WebSocket immediately. Both need the server to be listening ASAP.

## Design Space

These are options to evaluate — not recommendations. The right solution likely combines several.

### A. Early Bind, Late Initialize

Bind the TCP port immediately (before any initialization), return a "starting up" response on `/health`, accept WebSocket connections but hold messages. Finish initialization in background, then flip a readiness flag.

**Pro:** Server responds immediately, frontend can show a loading state.
**Con:** Need to handle requests arriving before initialization is complete. Routes, middleware, and AppState must all be constructable before skills/auth are ready.

### B. Two-Phase Startup

Split initialization into "essential" (DB, config, port bind, health endpoint) and "deferred" (skills, auth, agents). Essential phase completes in <1s. Deferred phase runs in background.

**Pro:** Clean separation. Health endpoint can report initialization progress.
**Con:** Requires careful dependency ordering. Some API endpoints must return 503 until deferred phase completes.

### C. Skill Loading Optimization

- **Single-pass walk:** Merge `walk_for_marker()` + `has_marker()` into one `read_dir()` call per directory.
- **Parallel I/O:** Use `rayon` or `tokio::spawn_blocking` to read + parse SKILL.md files concurrently.
- **Manifest cache:** Write a `skills.index` file mapping skill names → paths + hashes. On startup, validate hashes and only re-parse changed skills.
- **Lazy loading:** Don't parse SKILL.md contents at startup — only read the index. Parse on first access.

### D. Auth Cache Optimization

- **Concurrency limiter:** Use a `tokio::sync::Semaphore` to cap concurrent subprocess spawns (e.g., 10 at a time) instead of spawning all 137 at once.
- **Per-process timeout:** Add `tokio::time::timeout(Duration::from_secs(5), cmd.output())` around each subprocess.
- **Disk cache:** Persist auth cache to `~/.nebo/auth_cache.json` with TTL. On startup, use cached results and refresh in background.
- **Only check installed + enabled:** Skip auth checks for plugins that no enabled agent references.

### E. Deferred Agent Workers

Move agent worker startup (including auth cache) to after the server is listening. Workers can start in background. The first chat message to an agent triggers a "still initializing" response if workers aren't ready.

## Related Files

- `docs/bugs/skill-loading.md` — earlier log dump of skill loading issues (not structured documentation)
- `docs/bugs/skill-loader-scans-uninstalled-plugins.md` — related skill loader issue
