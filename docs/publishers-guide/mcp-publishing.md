# Publishing via MCP

This guide covers how to publish skills, plugins, and agents to NeboLoop using the MCP tools. All publishing operations go through the NeboLoop MCP server.

---

## Prerequisites

### Developer Account

Before uploading binaries or submitting artifacts for review, you must select a developer account:

```
developer(resource: account, action: list)
developer(resource: account, action: select, slug: "my-org")
```

The developer account stays selected for the duration of the MCP session. All subsequent operations use it.

### Namespace

Namespaces control where artifacts are published. List and select:

```
developer(resource: namespace, action: list)
developer(resource: namespace, action: select, slug: "my-org")
```

---

## Skills

Skills are markdown instructions (SKILL.md). No binary is required unless the skill bundles scripts or executables.

### Markdown-only Skills

1. **Create:**
   ```
   skill(action: create, name: "My Skill", category: "productivity", version: "1.0.0", manifestContent: "# My Skill\n\nInstructions here...")
   ```

2. **Update content:**
   ```
   skill(action: update, id: "<SKILL_ID>", manifestContent: "# Updated content...")
   ```

3. **Submit for review:**
   ```
   skill(action: submit, id: "<SKILL_ID>", version: "1.0.0")
   ```

### Skills with Binaries

Skills that bundle scripts or executables need binary uploads per platform.

1. **Create the skill:**
   ```
   skill(action: create, name: "My Skill", category: "productivity")
   ```

2. **Get upload token** (expires in 5 minutes):
   ```
   skill(action: binary-token, id: "<SKILL_ID>")
   ```
   Returns a curl command template.

3. **Upload binary + SKILL.md:**
   ```bash
   curl --http1.1 -X POST https://neboloop.com/api/v1/developer/apps/<SKILL_ID>/binaries \
     -H "Authorization: Bearer <TOKEN>" \
     -F "file=@/path/to/binary" \
     -F "platform=darwin-arm64" \
     -F "skill=@/path/to/SKILL.md"
   ```

4. **Repeat for each platform.** Valid platforms:
   - `darwin-arm64`
   - `darwin-amd64`
   - `linux-arm64`
   - `linux-amd64`
   - `windows-arm64`
   - `windows-amd64`

5. **Submit for review:**
   ```
   skill(action: submit, id: "<SKILL_ID>", version: "1.0.0")
   ```

### Managing Binaries

```
skill(action: list-binaries, id: "<SKILL_ID>")
skill(action: delete-binary, id: "<BINARY_ID>")
```

---

## Plugins

Plugins are native binaries with a PLUGIN.md manifest. They require a developer account and per-platform binary uploads.

### Publishing a Plugin

1. **Select developer account:**
   ```
   developer(resource: account, action: select, slug: "my-org")
   ```

2. **Create the plugin:**
   ```
   plugin(action: create, name: "My Plugin", category: "connectors")
   ```

3. **Get upload token:**
   ```
   plugin(action: binary-token, id: "<PLUGIN_ID>")
   ```

4. **Upload first platform with config and skills:**
   ```bash
   curl --http1.1 -X POST https://neboloop.com/api/v1/developer/apps/<PLUGIN_ID>/binaries \
     -H "Authorization: Bearer <TOKEN>" \
     -F "file=@dist/darwin-arm64/my-plugin" \
     -F "platform=darwin-arm64" \
     -F "skill=@PLUGIN.md" \
     -F "config=@plugin.json" \
     -F "skills=@/tmp/skills.tar.gz"
   ```

   | Form field | Required | Description |
   |------------|----------|-------------|
   | `file`     | yes      | The binary for this platform |
   | `platform` | yes      | Target platform (e.g., `darwin-arm64`) |
   | `skill`    | yes      | The PLUGIN.md manifest |
   | `config`   | no       | The plugin.json configuration |
   | `skills`   | no       | A `.tar.gz` of bundled SKILL.md files |

5. **Upload remaining platforms** (binary + platform + skill only):
   ```bash
   curl --http1.1 -X POST https://neboloop.com/api/v1/developer/apps/<PLUGIN_ID>/binaries \
     -H "Authorization: Bearer <TOKEN>" \
     -F "file=@dist/linux-amd64/my-plugin" \
     -F "platform=linux-amd64" \
     -F "skill=@PLUGIN.md"
   ```

   The `config` and `skills` fields only need to be sent once — they are not platform-specific.

6. **Submit for review:**
   ```
   plugin(action: submit, id: "<PLUGIN_ID>", version: "1.0.0")
   ```

### Skills Tarball

To bundle skills with a plugin, create a tar.gz from the skills directory:

```bash
tar -czf /tmp/skills.tar.gz -C skills .
```

The tarball should contain SKILL.md files at the root level (one per skill directory). Skills are imported on upload and the response includes a `skillsImported` count.

### Rebuilding

To delete and re-upload binaries (e.g., for a new release):

```
plugin(action: list-binaries, id: "<PLUGIN_ID>")
plugin(action: delete-binary, id: "<BINARY_ID>")
```

Then get a fresh upload token and re-upload. Upload tokens expire after 5 minutes.

---

## Agents

Agents have two files that get uploaded: `AGENT.md` (persona/manifest) and `agent.json` (operational wiring). They do **not** have platform-specific binaries.

### Publishing an Agent

1. **Select developer account:**
   ```
   developer(resource: account, action: select, slug: "my-org")
   ```

2. **Create the agent** (or update an existing one):
   ```
   skill(action: create, name: "My Agent", category: "productivity", manifestContent: "<AGENT.md content>")
   ```

   For updates:
   ```
   skill(action: update, id: "<AGENT_ID>", manifestContent: "<AGENT.md content>")
   ```

   > **Note:** Agents use the `skill()` MCP tool for marketplace CRUD, not the `agent()` tool. The `agent()` tool is for runtime AI agent configuration (autonomy levels, prompts, actions).

3. **Upload agent.json** — get a binary-token and upload via curl:
   ```
   skill(action: binary-token, id: "<AGENT_ID>")
   ```

   ```bash
   curl --http1.1 -X POST https://neboloop.com/api/v1/developer/apps/<AGENT_ID>/binaries \
     -H "Authorization: Bearer <TOKEN>" \
     -F "config=@/path/to/agent.json" \
     -F "platform=linux-amd64" \
     -F "skill=@/path/to/AGENT.md"
   ```

   | Form field | Required | Description |
   |------------|----------|-------------|
   | `config`   | yes      | The `agent.json` file — stored as `type_config` |
   | `platform` | yes      | Use `linux-amd64` (agents are not platform-specific, but the field is required) |
   | `skill`    | yes      | The `AGENT.md` manifest |

   **Critical:** The `config` field stores its contents as the agent's `type_config` in the database. This is where `agent.json` goes. Do **not** upload `manifest.json` as `config` — it will overwrite the `agent.json` data.

4. **Submit for review:**
   ```
   skill(action: submit, id: "<AGENT_ID>", version: "1.0.0")
   ```

### Verifying the Upload

After uploading, verify the agent.json was stored correctly:

```bash
curl -s "https://neboloop.com/api/v1/agents/<SLUG>" | python3 -c "
import sys, json
d = json.load(sys.stdin)
tc = d.get('typeConfig', {})
print('Keys:', list(tc.keys()))
print('Has workflows:', 'workflows' in tc)
"
```

The response should include the full agent.json structure (`inputs`, `workflows`, `skills`, `pricing`, `defaults`), not just the manifest.json keys (`name`, `version`, `description`).

### Common Mistakes

- **Uploading manifest.json as config:** The `config` form field maps to `type_config` in the database. If you upload `manifest.json` (3 keys: name, version, description) as `config`, it overwrites the `agent.json` (which has inputs, workflows, etc.). Only `agent.json` should go in the `config` field.

- **Invalid JSON:** The server validates that the `config` file is valid JSON. Trailing commas, missing brackets, or other syntax errors will return a 400 error. Validate locally first:
  ```bash
  python3 -c "import json; json.load(open('agent.json')); print('valid')"
  ```

- **Forgetting the platform field:** Even though agents are not platform-specific, the `platform` field is required by the upload endpoint. Use `linux-amd64` as the default.

---

## HTTP Upload Notes

- **Use `--http1.1`:** Large uploads (>10MB) can fail with HTTP/2 stream errors (exit code 92). Always use `--http1.1` with curl.

- **Token expiry:** Upload tokens expire after 5 minutes. Get a fresh token before each batch of uploads.

- **Parallel uploads:** Platform-specific uploads (plugins) can be run in parallel. The `config` and `skills` fields only need to be sent with one upload.

- **Duplicate key errors:** If a binary already exists for a version+platform combination, the upload returns a 500 with a duplicate key constraint error. Delete the existing binary first:
  ```
  skill(action: list-binaries, id: "<ID>")
  skill(action: delete-binary, id: "<BINARY_ID>")
  ```

---

## Quick Reference

### MCP Tool Mapping

| Artifact | CRUD Operations | Binary Uploads | Submit for Review |
|----------|----------------|----------------|-------------------|
| Skill    | `skill()`      | `skill(action: binary-token)` | `skill(action: submit)` |
| Plugin   | `plugin()`     | `plugin(action: binary-token)` | `plugin(action: submit)` |
| Agent    | `skill()`      | `skill(action: binary-token)` | `skill(action: submit)` |

### Curl Form Fields

| Field    | Skills | Plugins | Agents | Description |
|----------|--------|---------|--------|-------------|
| `file`   | yes    | yes     | no     | Binary file |
| `platform` | yes  | yes     | yes*   | Target platform |
| `skill`  | yes    | yes     | yes    | Markdown manifest (SKILL.md / PLUGIN.md / AGENT.md) |
| `config` | no     | optional | yes   | JSON config (plugin.json / agent.json) |
| `skills` | no     | optional | no    | Skills tarball (.tar.gz) |

\* Agents require `platform` but are not platform-specific. Use `linux-amd64`.

### Valid Platforms

`darwin-arm64`, `darwin-amd64`, `linux-arm64`, `linux-amd64`, `windows-arm64`, `windows-amd64`
