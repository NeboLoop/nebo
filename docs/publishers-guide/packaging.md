# Packaging & Marketplace

This guide covers the packaging format for all Nebo marketplace artifacts, artifact naming, and the artifact hierarchy.

For artifact-specific specs, see:

- [Skills](skills.md)
- [Platform Capabilities](platform-capabilities.md)
- [Workflows](workflows.md)
- [Roles](roles.md)
- [MCP Integrations](mcp.md)

## Hierarchy

```
ROLE  >  WORK  >  SKILL
(job)   (procedure) (knowledge + actions)
```

This is the **design direction** — the order in which you think about building. Start with knowledge and actions (Skill), chain those into procedures (Workflow), then compose procedures into a job (Role). The `>` represents conceptual priority, not a runtime dependency.

Each layer auto-installs its dependencies downward:

- **Role** → installs Workflows and Skills declared in `role.json`
- **Workflow** → installs Skills declared in `workflow.json` dependencies
- **Skill** → leaf node. No dependencies to auto-install.

**Platform Capabilities** (storage, network, vision, calendar, email, browser) are provided by Nebo itself — they are infrastructure, not marketplace artifacts. Skills declare which capabilities they need; the platform provides them. See [Platform Capabilities](platform-capabilities.md).

**Design direction:** Start with what the agent needs to *know* and *do* (Skill), chain skills into repeatable procedures (Workflow), and compose procedures into a job (Role). Skills are the universal unit — a folder with a SKILL.md file, optionally bundling scripts, reference docs, and assets. This is the same format used by the broader Agent Skills ecosystem (agentskills.io), so skills from Anthropic, OpenAI, OpenClaw, and other compatible platforms work in Nebo without modification.

---

## Artifact Naming

Every artifact has two identifiers: a **qualified name** (canonical, for engineering) and an **install code** (alias, for sharing).

### Qualified Names

The qualified name is the canonical identifier for every artifact. It is human-readable, org-scoped, type-aware, and versioned.

Format: `@org/type/name@version`

```
@acme/skills/sales-qualification@1.0.0
@acme/skills/crm-lookup@1.0.0
@acme/workflows/lead-qualification@2.1.0
@nebo/roles/chief-of-staff@1.0.0
```

Read left to right: who published it, what kind of artifact it is, what it's called, what version.

| Segment | Description | Rules |
|---------|-------------|-------|
| `@org` | Publisher org (scoped) | Lowercase, alphanumeric + hyphens |
| `type` | Artifact type | `skills`, `workflows`, `roles` |
| `name` | Artifact name | Lowercase, alphanumeric + hyphens |
| `@version` | Semver version (optional) | Omit for latest; supports semver ranges |

**Version resolution:**

| Reference | Resolves To |
|-----------|-------------|
| `@acme/skills/crm-lookup` | Latest published version |
| `@acme/skills/crm-lookup@1.0.0` | Exact version 1.0.0 |
| `@acme/skills/crm-lookup@^1.0.0` | Compatible with 1.0.0 (>=1.0.0 <2.0.0) |
| `@acme/skills/crm-lookup@>=1.0.0` | Any version 1.0.0 or higher |

Semver range semantics follow npm conventions. Exact versions pin to a specific release. Caret (`^`) allows patch and minor updates. Tilde (`~`) allows patch updates only.

**Namespacing:** Two publishers can build artifacts with the same name without collision:

```
@acme/skills/crm-lookup
@nebo/skills/crm-lookup
```

Different artifacts, no conflict. The org scope prevents name collisions as the marketplace grows.

### Install Codes (Aliases)

Install codes are short, shareable aliases assigned by the marketplace. They exist for one purpose: so a non-technical user can text a code to a friend and have it work.

Format: `PREFIX-XXXX-XXXX` — Crockford Base32 (`0123456789ABCDEFGHJKMNPQRSTVWXYZ`, excludes I, L, O, U), case-insensitive.

| Prefix | Artifact | Example |
|--------|----------|---------|
| `NEBO` | Link bot to NeboLoop account | `NEBO-A1B2-C3D4` |
| `SKIL` | Install a skill | `SKIL-R7KP-2M9V` |
| `WORK` | Install a workflow | `WORK-5TG2-XBJK` |
| `ROLE` | Install a role | `ROLE-9DCE-4MPA` |
| `LOOP` | Join bot to a Loop | `LOOP-7YSR-6WN3` |

Install codes always resolve to `@latest`. They are detected case-insensitively in chat messages and dispatched automatically.

**Install codes are marketing artifacts. Qualified names are engineering identifiers.** Inside `workflow.json`, `role.json`, dependency declarations, and all structured data — use qualified names. Install codes are for users, not for code.

---

## Package Format

### Skills — Directory Format (Agent Skills Standard)

Skills follow the [Agent Skills standard](https://agentskills.io). A skill is a **directory** containing a `SKILL.md` file and optional supporting resources. No manifest.json is required — the SKILL.md frontmatter is the source of truth.

```
skill-name/
├── SKILL.md           # Required — YAML frontmatter + markdown instructions
├── scripts/           # Optional — executable scripts (Python, TypeScript)
├── references/        # Optional — docs loaded on demand
├── assets/            # Optional — templates, images, fonts
└── examples/          # Optional — sample data, examples
```

Skills from Anthropic, OpenAI, OpenClaw, and other Agent Skills-compatible platforms can be installed directly with no modification. See [Skills](skills.md) for the full format spec.

**Marketplace distribution:** When a skill is submitted to NeboLoop, it is packaged as a signed `.napp` archive for integrity verification. At install time, Nebo reads the SKILL.md frontmatter and auto-generates internal registry metadata. The publisher never writes a manifest.json for skills.

**User/development path:** During development, place a skill directory directly in `user/skills/` and iterate. Hot-reload picks up changes with a 1-second debounce.

### Workflows and Roles — .napp Archive

Workflows and roles are distributed as `.napp` files — signed `tar.gz` archives.

```
@acme/workflows/lead-qualification-1.0.0.napp
  → manifest.json
  → workflow.json
  → WORKFLOW.md

@nebo/roles/chief-of-staff-1.0.0.napp
  → manifest.json
  → role.json
  → ROLE.md
```

### Sealed Archives

For workflows and roles, the `.napp` is **never extracted**. Nebo reads files directly from the archive at runtime. The signed archive is the running artifact — if someone tampers with the file, the next read fails signature verification. This provides continuous integrity, not just point-in-time verification at install.

| Artifact | Storage | Integrity Model |
|----------|---------|-----------------|
| Skill | Directory on disk (marketplace: sealed `.napp`) | Frontmatter is source of truth; marketplace archives are signed |
| Workflow | `.napp` sealed | Archive is the signed artifact — continuous integrity |
| Role | `.napp` sealed | Archive is the signed artifact — continuous integrity |

### Versioned Storage

Artifacts are stored by qualified name and version:

```
nebo/                                    # From NeboLoop marketplace
  skills/
    @acme/skills/sales-qualification/
      1.0.0.napp
      1.1.0.napp
  workflows/
    @acme/workflows/lead-qualification/
      1.0.0.napp
  roles/
    @nebo/roles/chief-of-staff/
      1.0.0.napp

user/                                    # User-created (dev/sideload path)
  skills/
    my-custom-skill/
      SKILL.md                           # Loose directory — no archive, no signature
      scripts/
      references/
  workflows/
    my-workflow/
      workflow.json
      WORKFLOW.md
  roles/
    my-role/
      role.json
      ROLE.md
```

**Marketplace artifacts** (`nebo/`) are sealed `.napp` files. Signed, versioned, read from archive at runtime.

**User artifacts** (`user/`) are loose files on disk. No archive, no signatures. This is the development path — edit directly, hot-reload picks up changes.

### Version Resolution

When a qualified name with a version range is referenced (e.g., `@acme/skills/sales-qualification@^1.0.0`), the resolution logic is:

1. Scan installed versions under that qualified name
2. Pick the highest version satisfying the semver range
3. If nothing matches, fetch from NeboLoop marketplace
4. If fetch fails and nothing is installed, error

This resolution applies everywhere a versioned qualified name appears — workflow `dependencies`, activity `skills` arrays, role `workflows` refs, and role-level `skills` arrays.

### Reading from Archives

Nebo reads `.napp` entries by name at runtime using a thin reader:

```rust
fn read_napp_entry(path: &Path, entry_name: &str) -> Result<Vec<u8>>
```

The skill loader reads `SKILL.md` from marketplace archives. The workflow engine calls `read_napp_entry(path, "workflow.json")`. The role loader calls `read_napp_entry(path, "role.json")` and `read_napp_entry(path, "ROLE.md")`. One function, used everywhere.

---

## manifest.json — Workflows and Roles Only

Workflows and roles ship a `manifest.json` as their marketplace identity envelope. **Skills do not use manifest.json** — their identity comes from SKILL.md frontmatter.

```json
{
  "name": "@acme/workflows/lead-qualification",
  "version": "1.0.0",
  "description": "Qualifies inbound leads through research, scoring, and routing",
  "signature": {
    "algorithm": "ed25519",
    "key_id": "nebo-prod-2026"
  }
}
```

### Fields

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `name` | string | yes | — | Qualified name: `@org/type/artifact-name` |
| `version` | string | yes | — | Semantic version |
| `description` | string | no | `""` | Short description |
| `tags` | string[] | no | `[]` | Categorization tags |
| `signature` | object | no | — | ED25519 signing metadata |

The `name` field is the canonical identifier. The org, type, and artifact name are all parsed from it. There is no separate `id`, `type`, or `author` field — the qualified name carries all of that.

> **Install codes** are not part of the package. They are assigned by NeboLoop when the manifest is submitted and are stored server-side as an alias that resolves to the qualified name. The publisher never sets the code — they submit their package and NeboLoop assigns one.

### Manifest Examples

**Workflow:**

```json
{
  "name": "@acme/workflows/lead-qualification",
  "version": "1.0.0",
  "description": "Qualifies inbound leads through CRM lookup, scoring, and routing"
}
```

**Role:**

```json
{
  "name": "@nebo/roles/chief-of-staff",
  "version": "1.0.0",
  "description": "Never be blindsided again — morning briefing, day monitoring, evening wrap"
}
```

---

## Separation of Concerns

Each artifact type has a clear split between identity, domain logic, and prose:

- **Skills** — `SKILL.md` frontmatter (identity + runtime config) + body (knowledge) + optional bundled resources (scripts, references, assets). No manifest.json. Frontmatter is the source of truth. Compatible with the Agent Skills standard.
- **Workflows** — `manifest.json` (marketplace identity) + `workflow.json` (procedure definition) + `WORKFLOW.md` (agent docs). Marketplace identity lives in the manifest; the workflow.json carries its own `id` for the local engine (REST API, run records, the `work` tool). Sealed archive.
- **Roles** — `manifest.json` (marketplace identity) + `role.json` (event bindings, pricing, defaults) + `ROLE.md` (persona prose). Sealed archive.

---

## Quick Reference

### Timeout Constants

| Operation | Timeout |
|-----------|---------|
| Hook call | 500 ms |
| Hook circuit breaker | 3 consecutive failures |
| Hook circuit breaker recovery | 5 minutes |
| Signing key cache | 24 hours |
| Revocation cache | 1 hour |

### Qualified Name Format

`@org/type/name@version` — org and name are lowercase alphanumeric + hyphens. Type is one of: `skills`, `workflows`, `roles`. Version follows semver.

### Install Code Format

`PREFIX-XXXX-XXXX` — Crockford Base32, case-insensitive. Always resolves to `@latest`.

Prefixes: `NEBO`, `SKIL`, `WORK`, `ROLE`, `LOOP`
