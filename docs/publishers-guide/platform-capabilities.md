# Platform Capabilities

Platform capabilities are infrastructure services provided by Nebo itself. They are not marketplace artifacts — they ship with the platform. Skills declare which capabilities they need; the platform provides them; users grant permission.

This is the same model as mobile app stores: iOS provides the camera API, apps declare they need camera access, the user grants permission once. Nebo provides storage, network, vision, and other capabilities. Skills declare what they need. Users grant permission when they install an Agent or Skill.

---

## Available Capabilities

| Capability | Description | What It Provides |
|------------|-------------|-----------------|
| `storage` | Persistent key-value storage | Read/write data that persists across sessions. Used for notes, state, configuration. |
| `network` | HTTP/API access | Make outbound HTTP requests to external APIs. Governed by allowlists. |
| `vision` | Image analysis | Analyze images — receipt scanning, screenshot interpretation, thumbnail evaluation. |
| `calendar` | Calendar access | Read and write calendar events, check availability, detect conflicts. |
| `email` | Email access | Read inbox, send emails, draft replies. |
| `browser` | Web browsing | Navigate web pages, extract content, fill forms. |
| `python` | Python runtime | Execute Python scripts in a sandboxed environment. |
| `typescript` | TypeScript runtime | Execute TypeScript/JavaScript in a sandboxed environment. |
| `notification` | User notifications | Send notifications to the user (desktop, mobile). |

---

## Declaring Capabilities

Skills declare which capabilities they need in their SKILL.md frontmatter:

```yaml
---
name: receipt-scanner
description: Extract data from receipt images and categorize expenses
capabilities: [vision, storage]
---
```

When a user installs a skill (or an Agent that includes it), Nebo shows which capabilities the skill requests and the user grants or denies access.

---

## Script Runtimes

The `python` and `typescript` capabilities provide sandboxed runtimes for executing scripts bundled with skills. Skills from the Agent Skills ecosystem (Anthropic, OpenAI, OpenClaw) frequently bundle Python and TypeScript scripts for tasks like document processing, data transformation, and API interaction.

### Sandbox Constraints

Scripts run in a restricted environment:

| Constraint | Policy |
|------------|--------|
| **Filesystem** | Read: skill's own directory + temp workspace. Write: temp workspace only. |
| **Network** | Denied by default. Allowed only if the skill also declares `network` capability. |
| **System calls** | Blocked — no spawning processes, no shell access. |
| **Time limit** | 30 seconds default (configurable per skill). |
| **Memory limit** | Capped per execution. |

### Declaring Script Dependencies

If a skill bundles Python scripts, declare `python` in capabilities:

```yaml
---
name: xlsx-processor
description: Create and edit Excel spreadsheets with formulas and formatting
capabilities: [python, storage]
---
```

If a skill bundles TypeScript scripts, declare `typescript`:

```yaml
---
name: api-tester
description: Test REST API endpoints and validate responses
capabilities: [typescript, network]
---
```

---

## Capability Permissions

Capabilities follow a deny-by-default model:

1. A skill declares `capabilities: [vision, storage]` in its frontmatter
2. At install time, Nebo shows the user what the skill requests
3. The user grants or denies each capability
4. At runtime, the platform enforces the granted permissions

If a skill tries to use a capability it didn't declare (or that the user denied), the operation fails with a clear error message.

---

## Why Platform Capabilities?

Skills are the marketplace artifact. Platform capabilities are Nebo's infrastructure. This separation creates two important properties:

1. **Security** — Nebo controls the security boundary. Skills are readable markdown + scripts, not compiled binaries from strangers. The platform owns the permission model.

2. **Portability** — Skills don't bundle their own storage layer, their own API client, or their own file access binary. They declare what they need and the platform provides it. This prevents the "16,000 skills, 386 containing malware" problem of ecosystems where every skill ships its own plumbing.

Publishers create **knowledge and composition** — how to do bookkeeping, how to manage clients, how to repurpose content. Nebo creates the **runtime** — secure execution, platform APIs, the install experience. Neither works without the other.
