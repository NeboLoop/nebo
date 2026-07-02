# Skills (`@org/skills/name`)

A Skill is a directory containing a `SKILL.md` file that teaches the agent how to handle a specific kind of request. Skills can also bundle scripts, reference docs, assets, and examples alongside the SKILL.md.

Nebo adopts the [Agent Skills standard](https://agentskills.io). Skills from Anthropic, OpenAI, OpenClaw, and other compatible platforms work in Nebo without modification.

For packaging and distribution, see [Packaging](packaging.md).

---

## Skill Directory Structure

A skill is a **folder** — not a single file. The SKILL.md is required; everything else is optional.

```
skill-name/
├── SKILL.md           # Required — YAML frontmatter + markdown instructions
├── scripts/           # Optional — executable scripts the agent can run
├── references/        # Optional — detailed docs loaded on demand
├── assets/            # Optional — templates, images, fonts, HTML
├── examples/          # Optional — sample data, example inputs/outputs
├── agents/            # Optional — subagent prompts
└── core/              # Optional — library code (Python packages, etc.)
```

### Progressive Disclosure

Skills use a three-level loading system:

1. **Metadata** (name + description from frontmatter) — Always in context (~100 words). This is what feeds skill *discovery* ranking — during a conversation the agent's own routing logic decides which skills to use.
2. **SKILL.md body** — Loaded when the skill triggers (<500 lines ideal). This is the knowledge injected into the agent's context.
3. **Bundled resources** — Loaded on demand. Scripts execute without being loaded into context. Reference docs are read only when the skill body points to them.

Keep SKILL.md under 500 lines. If approaching this limit, factor detailed content into `references/` files and point to them from the SKILL.md body.

---

## SKILL.md Format

```markdown
---
name: sales-email
description: Draft outbound sales emails matched to the prospect's company and role. Use when the user asks to write cold outreach, follow-up emails, or sales sequences.
---
# Sales Email Skill

You are a sales development representative. When the user asks you to
draft an outbound email, follow these steps:

1. Research the prospect's company and recent news
2. Draft a personalized email following the AIDA framework
3. Ask the user to review before sending

## Examples
- "Write a cold email to the VP of Engineering at Acme Corp"
- "Draft a follow-up to my meeting with Jane at TechCo"

## Guidelines
- Never use generic openers ("I hope this email finds you well")
- Reference something specific about their company
- Keep it under 150 words
```

---

## Frontmatter Fields

### Required (Agent Skills Standard)

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Skill identifier (lowercase, hyphens). This is the local name, not the qualified name. |
| `description` | string | yes | What the skill does and when to trigger it. This is the primary trigger mechanism — include both what the skill does AND specific contexts for when to use it. |

### Optional (Nebo Extensions)

These fields are Nebo-specific extensions. Other platforms ignore fields they don't recognize, so skills remain portable. The parser also recognizes the standard Agent Skills fields `license`, `compatibility`, and `allowed-tools` (alias `allowed_tools`).

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `version` | string | `1.0.0` | Semantic version |
| `author` | string | `""` | Skill author |
| `tags` | string[] | `[]` | Free-form tags for categorization and search |
| `capabilities` | string[] | `[]` | Platform capabilities the skill needs (see [Platform Capabilities](platform-capabilities.md)) |
| `triggers` | string[] | `[]` | Phrases used for discovery ranking (case-insensitive substring matching); they rank the skill, they do not activate it |
| `platform` | string[] | `[]` (all) | OS filter: `macos`, `linux`, `windows` |
| `priority` | int | `0` | Higher = matched first when multiple skills match |
| `max_turns` | int | `0` | Max agent turns (0 = unlimited) |
| `dependencies` | string[] | `[]` | Other skills' local `name` values this depends on (not qualified names). Verified at load time; if a dependency is missing, the skill is marked degraded (kept loaded but flagged via a warning); the skill still activates. |
| `requires` | object[] | `[]` | Skill-to-skill dependencies with version ranges. Each entry has `name` (the other skill's local `name`, not the qualified name) and `version` (semver range). |
| `plugins` | object[] | `[]` | Plugin dependencies (see [Plugin Dependencies](#plugin-dependencies)) |
| `metadata` | map | `{}` | Arbitrary key-value metadata |

### Capabilities

The `capabilities` field declares which platform capabilities the skill needs. The platform checks these against user-granted permissions.

```yaml
---
name: receipt-scanner
description: Extract data from receipt images and categorize expenses
capabilities: [vision, storage]
---
```

See [Platform Capabilities](platform-capabilities.md) for the full list of available capabilities and what each one provides.

---

## No manifest.json Required

Skills do NOT use manifest.json. The SKILL.md frontmatter is the source of truth for identity, configuration, and runtime behavior.

When a skill is submitted to NeboAI for marketplace distribution, Nebo auto-generates internal registry metadata from the frontmatter at install time. Publishers never write a manifest for skills.

This is what makes skills portable across platforms. A skill written for Claude Code, OpenAI Codex, or any Agent Skills-compatible platform drops into Nebo unchanged.

---

## Bundled Resources

### Scripts

Skills can bundle executable scripts that the agent runs during skill execution:

```
smart-scheduling/
├── SKILL.md
└── scripts/
    └── find_available_slots.py
```

The SKILL.md body tells the agent when and how to run the script:

```markdown
## Finding Available Slots

Use `scripts/find_available_slots.py` to check calendar availability:
```bash
python scripts/find_available_slots.py --date 2026-03-15 --duration 60
```

Script execution is provided by the platform's Python and TypeScript runtimes as platform capabilities. See [Platform Capabilities](platform-capabilities.md).

### References

Reference files are detailed docs loaded on demand — they stay out of context until the skill body explicitly tells the agent to read them:

```
mcp-builder/
├── SKILL.md
└── references/
    ├── python_mcp_server.md
    ├── node_mcp_server.md
    └── mcp_best_practices.md
```

The SKILL.md body directs the agent:

```markdown
## Implementation Guides
- For Python: read `references/python_mcp_server.md`
- For TypeScript: read `references/node_mcp_server.md`
```

For large reference files (>300 lines), include a table of contents at the top.

### Assets

Static resources used by the skill — templates, images, fonts, HTML files:

```
theme-factory/
├── SKILL.md
├── theme-showcase.pdf
└── themes/
    ├── modern-minimalist.md
    ├── ocean-depths.md
    └── sunset-boulevard.md
```

---

## Trigger Matching

The `triggers` and `description` fields are used for skill *discovery* ranking, not activation. They feed a scored discovery pass — `triggers` phrases are matched by **case-insensitive substring** against the user's message and, along with `description`, rank candidate skills (higher priority first, then by name). There is no trigger→activation gate: during a conversation the agent's own routing logic decides which skills to actually use.

The `description` field is the primary discovery signal. The `triggers` field in frontmatter provides additional phrase-matching as a Nebo extension.

> **Note on "triggers":** Skill triggers are NLP phrase-matching — case-insensitive substring matching against the user's message. This is a completely different mechanism from Agent triggers (schedule, heartbeat, event), which are event bindings. Same word, different systems.

---

## Platform Filtering

If `platform` is empty, the skill loads on all platforms. If specified, it only loads on the listed operating systems. Values are matched case-insensitively against the current OS (`macos`, `linux`, `windows`).

---

## Loading Order

Skills load in layered order — later sources override earlier ones by name:

1. **Embedded bundled skills** — compiled into the Nebo binary
2. **Installed skills** — extracted from `.napp` archives in `nebo/skills/`
   - **2.1 Sealed `.napp` skills** — a distinct sub-step that reads encrypted paid skills in memory only (never extracted to disk)
3. **Plugin-embedded skills** — skills bundled inside a plugin directory, loaded between the installed and user sources. The parent plugin slug is auto-injected as a required dependency.
   - **3.1 Marketplace-plugin-embedded skills** — from active marketplace plugins
   - **3.2 User-plugin-embedded skills** — from active user plugins (override marketplace-plugin skills)
4. **User skills** — loose files in `user/skills/`
5. **App skills** — loaded separately at app activation from an agent's `skills/` directory (overrides by name)

Skills are loaded from subdirectories — each skill lives in its own folder containing a `SKILL.md` file (case-insensitive filename matching).

## Hot-Reload

The skill loader watches the skills directory for file changes (create, modify, delete) with a 1-second debounce. When a change is detected, all skills are reloaded — bundled first, then user overrides. No restart needed.

---

## Complete Example: Briefing Writer

A focused skill designed for use within a single workflow activity — narrow, single-purpose, and domain-specific:

```markdown
---
name: briefing-writer
description: Synthesize information from calendar, email, and other sources into a concise morning briefing. Use when a workflow activity needs to produce a daily summary.
triggers:
  - write a briefing
  - morning summary
  - daily brief
capabilities: [storage]
priority: 10
max_turns: 3
---
# Briefing Writer

You synthesize information from multiple sources into a concise
morning briefing. Lead with the single most important thing.

## Process

1. Gather today's calendar events, pending messages, and overdue items
2. Flag any conflicts (overlapping times) and anything starting within 60 minutes
3. Synthesize into the format below

## What "important" means

- Time-sensitive beats interesting
- If two things compete, pick the one with a deadline
- Never surface something just because it's new

## Format

1. One sentence: the most important thing today
2. Calendar: what's on the schedule, conflicts, prep needed
3. Weather: only if it affects plans
4. Everything else: only if it matters
```

## Complete Example: Complex Skill with Scripts

A more complex skill that bundles executable scripts and references:

```
xlsx-processor/
├── SKILL.md
├── scripts/
│   └── recalc.py
└── references/
    └── formula-guide.md
```

```markdown
---
name: xlsx-processor
description: Create, edit, and analyze Excel spreadsheet files. Use when the user wants to open, modify, or create .xlsx files, add formulas, format cells, or work with tabular data.
capabilities: [python, storage]
---
# Excel Processing

Create and edit Excel files using openpyxl for formulas/formatting
or pandas for data analysis.

## Workflow
1. Choose tool: pandas for data, openpyxl for formulas/formatting
2. Create or load the workbook
3. Modify: add data, formulas, formatting
4. Save to file
5. Recalculate formulas: `python scripts/recalc.py output.xlsx`
6. Verify and fix any errors from the recalc output

## References
- For formula construction rules, see `references/formula-guide.md`
```

---

## Compatibility

Skills are portable across the Agent Skills ecosystem:

| Platform | Compatibility |
|----------|--------------|
| Nebo | Full support — frontmatter + body + bundled resources + Nebo extensions |
| Claude Code | Compatible — Nebo extensions in frontmatter are ignored |
| OpenAI Codex | Compatible — Nebo extensions in frontmatter are ignored |
| OpenClaw | Compatible — Nebo extensions in frontmatter are ignored |

The required fields (`name`, `description`) are universal. Nebo-specific fields (`capabilities`, `triggers`, `priority`, `max_turns`, `dependencies`) are silently ignored by other platforms.

---

## Sealed .napp Archives

Skills can be distributed as sealed `.napp` archives — encrypted, signed packages from the NeboAI marketplace. Sealed skills are read in memory only (never extracted to disk) and require a license key to decrypt.

- `persist_skill_from_api()` downloads and extracts skill content from the marketplace during install
- License keys are cached locally and re-injected at runtime for template loading
- Sealed skills appear in the catalog like any other skill; the encryption is transparent

For packaging details, see [Packaging](packaging.md).

---

## Plugin Dependencies

SKILL.md frontmatter can declare plugin dependencies using the `plugins` field:

```yaml
---
name: google-calendar
description: Manage Google Calendar events
plugins:
  - name: gws
    version: ">=1.2.0"
    optional: false
---
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Plugin slug (matches the plugin's registered name) |
| `version` | string | `"*"` | Semver version range (e.g. `">=1.2.0"`, `"^2.0.0"`) |
| `optional` | bool | `false` | If `true`, the skill loads without the plugin but features may be degraded |

When `optional` is `false` and the plugin is missing or doesn't satisfy the version range, the skill is marked degraded (kept loaded but flagged via a warning); the skill still activates. When `optional` is `true`, the skill loads but operates in a degraded mode.

Skills embedded inside a plugin directory automatically inherit the parent plugin as a required dependency.

---

## Template Variables

Skill SKILL.md body and bundled scripts can use template variables that Nebo expands at runtime. Variables use `${...}` syntax.

### Available Variables

| Variable | Example Value | Description |
|----------|---------------|-------------|
| `${NEBO_SKILL_DIR}` | `~/.nebo/nebo/skills/my-skill` | Directory containing the SKILL.md |
| `${NEBO_DATA_DIR}` | `~/.nebo/appdata/skills/my-skill` | Persistent data directory for this skill. Physically separated from the code directory — survives upgrades and reinstalls. Created lazily on first use. |
| `${NEBO_USER_NAME}` | `Alex` | User's configured display name |
| `${NEBO_OS}` | `macos` | Operating system (`macos`, `linux`, or `windows`) |
| `${NEBO_ARCH}` | `aarch64` | CPU architecture (e.g. `aarch64`, `x86_64`) |
| `${plugin.SLUG_BIN}` | `/path/to/gws` | Resolved binary path for a plugin dependency. `SLUG` is uppercased with hyphens replaced by underscores (e.g. `gws` → `${plugin.GWS_BIN}`) |
| `${secret.KEY}` | `sk-abc123...` | Decrypted secret value. Secrets are configured per-skill in Settings > Secrets. |

### Usage in SKILL.md

```markdown
## Calendar Sync

Run the sync script to fetch today's calendar:
```bash
${plugin.GWS_BIN} calendar list --date today --output ${NEBO_DATA_DIR}/calendar.json
```

Check the output:
```bash
cat ${NEBO_DATA_DIR}/calendar.json
```
```

### Data Directory

The `${NEBO_DATA_DIR}` directory is **separate from the skill's code directory**. It lives at `<data_dir>/appdata/skills/<name>/` (typically `~/.nebo/appdata/skills/<name>/`), not inside the skill folder. This means:

- Upgrading the skill (replacing SKILL.md, scripts, etc.) never touches data
- Uninstalling and reinstalling preserves data
- The skill owns its own data migrations across versions

Store databases, caches, generated files, and any persistent state here — never in `${NEBO_SKILL_DIR}`.

---

## Concurrency Safety

Read-only skill tool actions run in parallel: `list`, `discover`, `help`, `browse`, `read_resource`, `reviews`, `secrets`. These are the actions allowlisted by the per-action `is_concurrent_safe()` check.

Write actions (`install`, `create`, `update`, `delete`, `configure`, `load`, `unload`, `rate`) are simply excluded from the concurrent-safe allowlist, so they run non-concurrently. The loader's internal `RwLock` guards the in-memory skills map; it does not serialize write actions.

---

## Skill Tool Actions Reference

In addition to the actions documented above (`list`, `discover`, `help`, `browse`, `read_resource`, `load`, `unload`, `create`, `update`, `delete`, `install`, `configure`):

| Action | Description |
|--------|-------------|
| `secrets` | Shows configured vs missing secrets for a skill. Lists each declared secret with its status (configured, MISSING, or not set). |
| `reviews` | Fetches live marketplace reviews for a skill from the NeboAI API. |
| `rate` | Submits a 1–5★ review for a skill (mutating — posts to the marketplace). |

---

## Key Design Points

- **Narrow and focused** — each skill should do one thing well
- **Knowledge + actions** — the body teaches the agent what to know; bundled scripts give it tools to act
- **Portable** — write once, use on any Agent Skills-compatible platform
- **Progressive disclosure** — metadata always loaded, body on trigger, resources on demand
- **Platform capabilities** — declare what infrastructure the skill needs (storage, network, vision); the platform provides it
