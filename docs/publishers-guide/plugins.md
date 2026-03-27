# Plugins (`@org/plugins/name`)

A Plugin is a managed native binary that skills depend on. Instead of bundling a heavy binary inside every skill that needs it, publish the binary once as a plugin and let skills declare it as a dependency. Nebo downloads the plugin automatically when a skill that needs it is installed.

For packaging and distribution, see [Packaging](packaging.md).

---

## When to Use a Plugin vs. Embedded Binary

| Pattern | Use When | Example |
|---------|----------|---------|
| **Plugin** | Multiple skills share the same binary, or the binary is large (>5MB) | `gws` (Google Workspace CLI), `ffmpeg` |
| **Embedded binary** | One skill bundles its own small binary | Custom tool specific to a single skill |

You can use both patterns. A skill can embed a binary AND declare plugin dependencies. The embedded binary takes precedence for the skill's own execution; plugin binaries are available to scripts via environment variables.

---

## How It Works

1. Publisher uploads a native binary to NeboLoop for each platform
2. Publisher creates a skill with `plugins:` in SKILL.md frontmatter
3. User installs the skill (via marketplace or `SKIL-XXXX-XXXX` code)
4. Nebo detects the plugin dependency and downloads the binary silently
5. Binary is stored locally at `<data_dir>/nebo/plugins/<slug>/<version>/`
6. Skill scripts access the binary via `{SLUG}_BIN` environment variable

```
User installs skill → SKILL.md declares plugins: [{name: "gws", version: ">=1.2.0"}]
  → Nebo downloads gws binary for current platform
  → Skill script runs with GWS_BIN=/path/to/gws
```

---

## Declaring Plugin Dependencies

Add a `plugins` field to your SKILL.md frontmatter:

```yaml
---
name: google-workspace
description: Manage Google Workspace — Gmail, Calendar, Drive
plugins:
  - name: gws
    version: ">=1.2.0"
---
```

### Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Plugin slug (must match the plugin's registered slug in NeboLoop) |
| `version` | string | `"*"` | Semver version range |
| `optional` | bool | `false` | If true, the skill loads even if this plugin isn't installed |

### Version Ranges

Version ranges follow semver conventions:

| Range | Meaning |
|-------|---------|
| `"*"` | Any version |
| `">=1.2.0"` | 1.2.0 or higher |
| `"^1.0.0"` | Compatible with 1.x.x (>=1.0.0, <2.0.0) |
| `"~1.2.0"` | Patch updates only (>=1.2.0, <1.3.0) |
| `"=1.2.0"` | Exact version |

### Multiple Dependencies

```yaml
---
name: media-processor
description: Process and convert media files
plugins:
  - name: ffmpeg
    version: ">=5.0.0"
  - name: imagemagick
    version: ">=7.0.0"
    optional: true
---
```

The skill loads only if all **required** plugins resolve. Optional plugins are silently skipped if missing.

---

## Using Plugin Binaries in Scripts

Plugin binaries are exposed to your scripts as environment variables. The naming convention is `{SLUG}_BIN` where the slug is uppercased and hyphens become underscores.

| Plugin Slug | Environment Variable |
|-------------|---------------------|
| `gws` | `GWS_BIN` |
| `ffmpeg` | `FFMPEG_BIN` |
| `my-tool` | `MY_TOOL_BIN` |

### Python Example

```python
#!/usr/bin/env python3
import os
import subprocess

gws_bin = os.environ["GWS_BIN"]
result = subprocess.run([gws_bin, "gmail", "list", "--limit", "10"], capture_output=True, text=True)
print(result.stdout)
```

### TypeScript Example

```typescript
import { execSync } from "child_process";

const gwsBin = process.env.GWS_BIN!;
const output = execSync(`${gwsBin} gmail list --limit 10`, { encoding: "utf-8" });
console.log(output);
```

### Shell Example

```bash
#!/bin/bash
$GWS_BIN gmail list --limit 10
```

---

## Publishing a Plugin

### Prerequisites

- A NeboLoop developer account
- Your binary compiled for at least one target platform
- Binary must be a single executable file (no runtime dependencies)

### Supported Platforms

| Platform Key | OS | Architecture |
|-------------|-----|--------------|
| `darwin-arm64` | macOS | Apple Silicon |
| `darwin-amd64` | macOS | Intel |
| `linux-arm64` | Linux | ARM64 |
| `linux-amd64` | Linux | x86_64 |
| `windows-arm64` | Windows | ARM64 |
| `windows-amd64` | Windows | x86_64 |

Publish for as many platforms as you support. At minimum, target `darwin-arm64` and `linux-amd64`.

### Step-by-Step

1. **Select your developer account:**

   ```
   developer(resource: account, action: select, id: "<your-dev-account-id>")
   ```

2. **Create the plugin artifact:**

   ```
   skill(action: create, name: "gws", type: "plugin")
   ```

   Note: Plugins use the `skill` tool with `type: "plugin"` on NeboLoop. The returned ID is your plugin artifact ID.

3. **Get an upload token (per platform):**

   ```
   skill(action: binary-token, id: "<PLUGIN_ID>")
   ```

   This returns a curl command with a 5-minute expiry.

4. **Upload binary + manifest per platform:**

   Use the returned curl command via the command line, replacing the file path and platform:

   ```bash
   curl -X PUT "<upload-url>" \
     -F "binary=@./build/gws-darwin-arm64" \
     -F "platform=darwin-arm64" \
     -F "manifest=@./SKILL.md"
   ```

   Repeat for each platform you support.

5. **Submit for review:**

   ```
   skill(action: submit, id: "<PLUGIN_ID>", version: "1.0.0")
   ```

### Install Codes

After your plugin is approved, NeboLoop assigns a `PLUG-XXXX-XXXX` install code. Users can paste this code into Nebo's chat to install the plugin directly. However, plugins are typically installed as dependencies of skills — users rarely install plugins standalone.

---

## Directory Structure

Unlike skills, plugins don't have a directory structure for publishers to maintain. You just compile your binary and upload it. NeboLoop handles the rest.

What the user sees on disk after install:

```
<data_dir>/nebo/plugins/
  gws/
    1.2.0/
      plugin.json      # Cached manifest (auto-generated)
      gws              # Your binary (chmod 755)
```

---

## Security

- **SHA256 verification:** Every binary is hashed on upload. On download, the hash is verified before the binary is written to disk. Any mismatch = download rejected.
- **ED25519 signatures:** Binaries are signed with NeboLoop's ED25519 key. Signatures are verified on download when the signing key is available.
- **Quarantine:** If a plugin is revoked (security issue, policy violation), Nebo deletes the binary and writes a `.quarantined` marker. The plugin becomes unresolvable, and any skills depending on it are dropped from the loaded set.
- **No network required after install:** Once downloaded, `resolve()` is fully local. Works offline.

---

## Versioning and Updates

- Multiple versions of the same plugin can coexist on disk (e.g., `gws/1.2.0/` and `gws/1.3.0/`)
- Each skill resolves to the highest installed version matching its semver range
- When you publish a new version, users get it automatically the next time Nebo resolves the dependency
- Old versions are cleaned up by garbage collection when no skill references them

### Garbage Collection

Nebo periodically checks which plugin slugs are referenced by loaded skills. Unreferenced plugin directories are removed. This is deferred (not eager) — plugins aren't deleted the moment a skill is uninstalled.

---

## Testing During Development

During development, you can manually place a plugin binary in the expected directory structure:

```bash
mkdir -p ~/Library/Application\ Support/nebo/plugins/my-plugin/0.1.0/
cp ./build/my-plugin ~/Library/Application\ Support/nebo/plugins/my-plugin/0.1.0/
chmod 755 ~/Library/Application\ Support/nebo/plugins/my-plugin/0.1.0/my-plugin
```

Then create a skill with `plugins: [{name: "my-plugin", version: "*"}]` in your `user/skills/` directory. The loader will resolve the plugin locally without contacting NeboLoop.

---

## Quick Reference

### Environment Variable Naming

`{SLUG}_BIN` — slug uppercased, hyphens → underscores.

### Install Code Prefix

`PLUG-XXXX-XXXX` — Crockford Base32, case-insensitive.

### SKILL.md Frontmatter

```yaml
plugins:
  - name: <slug>          # Required
    version: "<range>"    # Optional, default "*"
    optional: <bool>      # Optional, default false
```

### NeboLoop Qualified Name

```
@org/plugins/name@version
```

Same scoping and version resolution rules as skills. See [Packaging](packaging.md).
