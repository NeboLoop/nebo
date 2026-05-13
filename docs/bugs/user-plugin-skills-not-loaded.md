# Bug: Skills Embedded in User Plugins Are Not Loaded or Watched

## Summary

When a plugin is installed to the **user plugins directory** (`<data_dir>/user/plugins/<slug>/<version>/`), its embedded skills (in `skills/` subdirectories) are never loaded by the skills `Loader`. Only skills from **marketplace plugins** (`<data_dir>/nebo/plugins/`) are scanned. This means user-installed or locally-developed plugins with embedded skills are invisible to the agent.

## Root Cause

Two gaps in `crates/tools/src/skills/loader.rs`:

### 1. `load_all()` only scans marketplace plugins dir (line 82-109)

The "2.5" step that loads skills embedded in plugins calls `ps.plugins_dir()` which returns `installed_dir` (marketplace). It never calls `ps.user_plugins_dir()` to scan the user plugins directory.

```rust
// Line 82-109 — only scans ps.plugins_dir() (marketplace)
if let Some(ref ps) = self.plugin_store {
    let plugins_dir = ps.plugins_dir();  // ← BUG: only marketplace dir
    // ...scans for skills in plugin subdirs...
}
```

**Should also scan `ps.user_plugins_dir()`** with the same logic, and user plugins should override marketplace plugins (matching the existing priority: bundled → installed → plugins → user).

### 2. `watch()` only watches marketplace plugins dir (line 284-290)

The filesystem watcher captures `plugins_dir` from the plugin store but never captures or watches `user_plugins_dir`:

```rust
// Line 249 — only captures marketplace dir
let plugins_dir = plugin_store.as_ref().map(|ps| ps.plugins_dir().to_path_buf());
```

The watcher setup (line 284-290) and the reload logic (line 362-369) both only reference this single `plugins_dir`.

**Should also watch `user_plugins_dir`** and include it in the reload scan.

## Files to Change

### `crates/tools/src/skills/loader.rs`

#### Fix 1: `load_all()` — Add user plugins scanning after marketplace plugins

After the existing "2.5" block (lines 81-109), add a nearly identical block that scans `ps.user_plugins_dir()`. This should come **after** marketplace plugins but **before** user loose skills (step 3), so user plugin skills override marketplace plugin skills but are themselves overridden by loose user skills.

The logic is identical to the existing block:
- Iterate slug directories under `user_plugins_dir`
- For each, call `load_skills_from_nested_dir()` with `SkillSource::Installed`
- Auto-inject the parent plugin slug as a `PluginDependency`
- Insert into `loaded` (overriding by name)

#### Fix 2: `watch()` — Add user plugins dir to watcher and reload

1. **Capture the user plugins dir** alongside the marketplace dir (around line 249):
```rust
let user_plugins_dir = plugin_store.as_ref().map(|ps| ps.user_plugins_dir().to_path_buf());
```

2. **Watch the user plugins dir** (after line 290, add a similar block):
```rust
if let Some(ref updir) = user_plugins_dir {
    if updir.exists() {
        if let Err(e) = watcher.watch(updir, RecursiveMode::Recursive) {
            warn!(error = %e, dir = %updir.display(), "failed to watch user plugins dir for skills");
        }
    }
}
```

3. **Include user plugins in the reload scan** (after line 369, add the same nested dir scan for `user_plugins_dir` with auto-injected plugin dependency, matching the `load_all()` fix).

## Existing Methods Available

- `PluginStore::user_plugins_dir()` → returns `&Path` to `<data_dir>/user/plugins/` (already exists, line 178 of `plugin.rs`)
- `PluginStore::plugins_dir()` → returns `&Path` to `<data_dir>/nebo/plugins/` (marketplace)
- `load_skills_from_nested_dir(dir, source)` → recursively walks a dir for SKILL.md files
- `PluginStore::resolve()` already checks user dir first, then installed — the binary resolution is correct; only skills loading is broken

## How to Verify

1. Build the outreach plugin and install it to `~/Library/Application Support/Nebo/user/plugins/outreach/0.1.0/` with a `skills/` subdirectory containing SKILL.md files.
2. Start Nebo — the outreach skills should appear in `skill(action: "catalog")`.
3. Modify a SKILL.md in the user plugins dir — skills should hot-reload within ~2 seconds.

## Test

Add a test in `loader.rs` that creates a mock `PluginStore` with a `user_dir` containing a plugin with embedded skills, and verifies `load_all()` picks them up. The existing test infrastructure (`TempDir`, `create_skill_md`) can be extended for this.

## Notes

- The `PluginStore` itself works correctly — `resolve()` checks user dir first, `list_installed()` scans both dirs, `build_env_map()` includes user plugins. The gap is only in the skills `Loader`.
- The `plugin_inventory()` method on `Loader` (line 212-233) calls `ps.build_env_map()` which already includes user plugins, so the agent's system prompt already lists user plugins. The agent just can't load their skills.
- Priority order should remain: bundled → installed (marketplace skills) → marketplace plugin skills → **user plugin skills** → user loose skills. Each layer overrides by name.
