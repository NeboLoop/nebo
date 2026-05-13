# Bug: Skill Loader Scans All Plugin Directories Regardless of Installation Status

## Summary

The skill loader in `crates/tools/src/skills/loader.rs` scans **every** plugin directory under the marketplace plugins path and loads embedded skills — even for plugins that are not installed. Since the loaded skills have auto-injected plugin dependencies that can't be resolved (the plugin binary isn't present), `verify_dependencies()` marks them as `degraded`. This produces **~1,700 "skill degraded: unmet dependencies" warnings** on every startup, flooding the log and obscuring real issues.

## Root Cause

In `loader.rs` lines 82–109 (the "2.5" step in `load_all()`), the loader iterates over all slug directories under `ps.plugins_dir()` and loads any `SKILL.md` files found. It auto-injects the parent plugin slug as a `PluginDependency` on each skill. The problem: **it never checks whether the plugin is actually installed** before loading its skills.

```rust
// loader.rs ~line 82-109
if let Some(ref ps) = self.plugin_store {
    let plugins_dir = ps.plugins_dir();
    if plugins_dir.exists() {
        // Iterates ALL directories — installed or not
        for slug_entry in std::fs::read_dir(plugins_dir)... {
            // Loads skills and auto-injects plugin dependency
            // Never checks if this plugin is actually installed
        }
    }
}
```

Later, `verify_dependencies()` (lines 927–1008) calls `plugin_store.resolve()` for each skill's plugin dependency. For uninstalled plugins, `resolve()` returns `None`, and the skill is marked `degraded`:

```
WARN skill degraded: unmet dependencies skill="some-plugin-skill" missing=["PLUG-XXXX-YYYY"]
```

The marketplace plugins directory contains metadata/manifests for all available plugins (188+), not just installed ones. The loader doesn't distinguish between "available" and "installed."

## Impact

- **~1,700 degradation warnings** per startup (each uninstalled plugin's skills × their dependencies)
- Log noise makes it hard to spot real skill loading issues
- Slight startup performance hit from loading and verifying skills that will never be usable
- No functional impact on installed plugins — their skills load and verify correctly

## Suggested Fix

### Option A: Filter during loading (recommended — minimal change)

Before loading skills from a plugin directory, check if the plugin is actually installed via `plugin_store.resolve()`:

```rust
if let Some(ref ps) = self.plugin_store {
    let plugins_dir = ps.plugins_dir();
    if plugins_dir.exists() {
        for slug_entry in std::fs::read_dir(plugins_dir)... {
            let slug = slug_entry.file_name().to_string_lossy().to_string();

            // Skip plugins that aren't installed
            if ps.resolve(&slug).is_none() {
                continue;
            }

            // Load skills from this plugin's directory...
        }
    }
}
```

This is the smallest change — one guard clause. Skills for uninstalled plugins are simply skipped.

### Option B: Filter during verification

Instead of preventing loading, change `verify_dependencies()` to silently skip (or remove) skills whose parent plugin isn't installed, rather than logging a warning. Less ideal because it still does unnecessary loading work.

## Files

- **`crates/tools/src/skills/loader.rs`** — `load_all()` lines 82–109 (loading), `verify_dependencies()` lines 927–1008 (warning source)
- **`crates/tools/src/skills/skill.rs`** — `PluginDependency` struct (lines 19–39)
- **`crates/napp/src/plugin.rs`** — `PluginStore::resolve()` (lines 472–567), used to check installation status

## Related

- `user-plugin-skills-not-loaded.md` — the companion bug where user plugin skills are never scanned at all
- `crates/server/src/deps.rs` — the install cascade resolver (lines 64–200) that handles installing plugin dependencies when an agent is installed; this works correctly, the issue is purely in the skill loader's scan scope

## How to Verify

1. Apply the fix
2. Start Nebo with only a few plugins installed
3. Confirm only installed plugins' skills appear in `skill(action: "catalog")`
4. Confirm zero "skill degraded: unmet dependencies" warnings for plugins the user never installed
5. Install a new plugin → its skills should appear after the install cascade completes
