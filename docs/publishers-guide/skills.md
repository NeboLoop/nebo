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

1. **Metadata** (name + description from frontmatter) — Always in context (~100 words). This is what determines whether the skill triggers.
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

These fields are Nebo-specific extensions. Other platforms ignore fields they don't recognize, so skills remain portable.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `version` | string | `1.0.0` | Semantic version |
| `capabilities` | string[] | `[]` | Platform capabilities the skill needs (see [Platform Capabilities](platform-capabilities.md)) |
| `triggers` | string[] | `[]` | Phrases that activate the skill (case-insensitive substring matching) |
| `platform` | string[] | `[]` (all) | OS filter: `macos`, `linux`, `windows` |
| `priority` | int | `0` | Higher = matched first when multiple skills match |
| `max_turns` | int | `0` | Max agent turns (0 = unlimited) |
| `dependencies` | string[] | `[]` | Other skill qualified names this depends on. Verified at load time. |
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

When a skill is submitted to NeboLoop for marketplace distribution, Nebo auto-generates internal registry metadata from the frontmatter at install time. Publishers never write a manifest for skills.

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

Triggers are matched by **case-insensitive substring** against the user's message. If the message contains any trigger phrase, the skill is activated. When multiple skills match, they are sorted by priority (highest first), then by name.

The `description` field is the primary trigger mechanism. The `triggers` field in frontmatter provides additional phrase-matching as a Nebo extension.

> **Note on "triggers":** Skill triggers are NLP phrase-matching — case-insensitive substring matching against the user's message. This is a completely different mechanism from Agent triggers (schedule, heartbeat, event), which are event bindings. Same word, different systems.

---

## Platform Filtering

If `platform` is empty, the skill loads on all platforms. If specified, it only loads on the listed operating systems. Values are matched case-insensitively against the current OS (`macos`, `linux`, `windows`).

---

## Loading Order

1. **Bundled skills** load first (shipped with Nebo)
2. **Installed skills** load second (from marketplace)
3. **User skills** load last and **override** bundled or installed skills with the same name

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

## Key Design Points

- **Narrow and focused** — each skill should do one thing well
- **Knowledge + actions** — the body teaches the agent what to know; bundled scripts give it tools to act
- **Portable** — write once, use on any Agent Skills-compatible platform
- **Progressive disclosure** — metadata always loaded, body on trigger, resources on demand
- **Platform capabilities** — declare what infrastructure the skill needs (storage, network, vision); the platform provides it
