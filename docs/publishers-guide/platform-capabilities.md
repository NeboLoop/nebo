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

## Capability Details

### `storage` — Persistent Key-Value Store

Provides per-skill persistent storage at `${NEBO_DATA_DIR}`. This directory is physically separated from the skill's code — it lives at `~/.nebo/appdata/skills/<name>/` and survives upgrades, reinstalls, and Nebo updates.

**What you can store:**
- JSON files, SQLite databases, cached API responses
- User-generated content (notes, exports, processed data)
- Configuration state and preferences

**How to use it:**
- In SKILL.md body, reference `${NEBO_DATA_DIR}` — Nebo expands this to the absolute path
- In scripts, read `${NEBO_DATA_DIR}` from the expanded command or environment
- The directory is created lazily on first reference

```markdown
## Saving Results

Save analysis output for later reference:
```bash
python scripts/analyze.py --output ${NEBO_DATA_DIR}/report.json
```
```

**Constraints:**
- No size limit enforced, but skills should be good citizens (clean up temp files)
- Data is local to the machine — not synced across devices
- Multiple skills each get their own isolated data directory

---

### `network` — Outbound HTTP Access

Allows skills to make HTTP requests to external APIs. Without this capability, all outbound network access is blocked.

**What it provides:**
- HTTP GET/POST/PUT/DELETE to external APIs
- Access through the agent's web tool (`web` domain tool)
- URL fetch and content extraction

**How to use it:**
- The agent's built-in `web` tool handles HTTP requests when the skill is active
- Scripts can make HTTP requests via standard libraries (requests, fetch, etc.)

```yaml
---
name: stock-checker
description: Check stock prices and market data from financial APIs
capabilities: [network]
---
```

**Constraints:**
- Governed by domain allowlists configured by the user
- No raw socket access — HTTP/HTTPS only
- Rate limiting may apply per domain

---

### `vision` — Image Analysis

Enables the agent to analyze images — screenshots, photos, scanned documents, diagrams.

**What it provides:**
- Image content description and analysis
- Text extraction from images (OCR)
- Visual comparison and assessment
- Screenshot interpretation

**How to use it:**
- The agent receives images as multimodal input and can describe/analyze them
- Skills can instruct the agent to interpret specific visual elements

```yaml
---
name: receipt-scanner
description: Extract line items and totals from receipt photos
capabilities: [vision, storage]
---
```

**Constraints:**
- Supported formats: PNG, JPG, GIF, WebP, SVG
- Processing uses the configured LLM's vision capabilities

---

### `calendar` — Calendar Integration

Read and write calendar events. Requires an active calendar plugin (e.g., Google Workspace).

**What it provides:**
- List events for a date range
- Create new calendar events
- Check availability and detect scheduling conflicts
- Access attendee information

**How to use it:**
- The agent uses the calendar through the plugin system
- Skills declare `calendar` to indicate they need calendar access
- Actual calendar data flows through the configured calendar plugin (GWS, Outlook, etc.)

```yaml
---
name: meeting-scheduler
description: Find optimal meeting times and create calendar invites
capabilities: [calendar]
plugins:
  - name: gws
    version: ">=1.0.0"
---
```

---

### `email` — Email Integration

Read and send emails. Requires an active email plugin.

**What it provides:**
- Read inbox messages
- Send new emails and replies
- Draft emails for user review
- Search email history

**How to use it:**
- The agent accesses email through the plugin system
- Skills declare `email` to indicate email access is needed
- Email data flows through the configured email plugin

```yaml
---
name: email-assistant
description: Summarize inbox, draft replies, and manage email workflow
capabilities: [email]
plugins:
  - name: gws
    version: ">=1.0.0"
---
```

---

### `browser` — Web Browsing

Navigate web pages, extract content, and interact with web forms.

**What it provides:**
- Navigate to URLs and extract page content
- Fill forms and click elements (via browser automation)
- Extract structured data from web pages
- Screenshot web pages

**How to use it:**
- The agent uses the `web` domain tool's browser capabilities
- Skills declare `browser` to indicate they need interactive web access beyond simple HTTP

```yaml
---
name: web-researcher
description: Research topics by browsing multiple sources and synthesizing findings
capabilities: [browser, network]
---
```

**Constraints:**
- Browser sessions are sandboxed
- No access to the user's browser profile, cookies, or saved passwords
- Pages are loaded in a headless browser environment

---

### `python` — Python Script Runtime

Execute Python scripts bundled with the skill in a sandboxed environment.

**What it provides:**
- Python 3.x interpreter
- Access to the skill's `scripts/` directory
- Standard library modules

**How to use it:**
- Bundle `.py` files in your skill's `scripts/` directory
- Reference them from your SKILL.md body
- The agent runs scripts when instructed by the skill

```yaml
---
name: xlsx-processor
description: Create and edit Excel spreadsheets with formulas and formatting
capabilities: [python, storage]
---
```

```markdown
## Processing Excel Files

Create a spreadsheet:
```bash
python scripts/create_workbook.py --output ${NEBO_DATA_DIR}/report.xlsx
```
```

**Sandbox constraints:**

| Constraint | Policy |
|------------|--------|
| **Filesystem read** | Skill's own directory (`${NEBO_SKILL_DIR}`) + temp workspace |
| **Filesystem write** | Data directory (`${NEBO_DATA_DIR}`) + temp workspace only |
| **Network** | Denied unless skill also declares `network` capability |
| **Process spawning** | Blocked — no `subprocess`, no shell access |

---

### `typescript` — TypeScript/JavaScript Runtime

Execute TypeScript or JavaScript scripts bundled with the skill.

**What it provides:**
- Node.js/Deno runtime
- Access to the skill's `scripts/` directory
- Standard modules

**How to use it:**

```yaml
---
name: api-tester
description: Test REST API endpoints and validate responses
capabilities: [typescript, network]
---
```

Same sandbox constraints as Python — filesystem, network, and process restrictions apply.

---

### `notification` — User Notifications

Send notifications to the user through desktop or mobile channels.

**What it provides:**
- Desktop notifications (macOS, Windows, Linux native)
- Notification scheduling
- Actionable notifications (buttons, quick replies)

**How to use it:**
- The agent sends notifications through the `message` domain tool
- Skills declare `notification` to indicate they may alert the user

```yaml
---
name: deadline-tracker
description: Monitor project deadlines and alert when due dates approach
capabilities: [notification, storage]
---
```

---

## Capability Permissions

Capabilities follow a declare-and-enforce model:

1. A skill declares `capabilities: [vision, storage]` in its frontmatter
2. At install time, Nebo shows the user which capabilities the skill requires
3. At runtime, declared capabilities translate into sandbox configuration — a skill with `storage` gets write access to `${NEBO_DATA_DIR}`, a skill with `network` gets outbound HTTP access, etc.
4. Undeclared capabilities are not available (e.g., a skill without `network` cannot make outbound HTTP requests from scripts)

Skills should be written to handle missing capabilities gracefully — if a required capability's underlying service is unavailable, the skill should inform the user rather than failing silently.

---

## Combining Capabilities

Skills frequently need multiple capabilities. Common combinations:

| Use Case | Capabilities | Why |
|----------|-------------|-----|
| Document processing | `python`, `storage` | Scripts process files, results persist |
| API integration | `network`, `storage` | Fetch external data, cache locally |
| Web research | `browser`, `network`, `storage` | Browse pages, save findings |
| Calendar management | `calendar`, `notification` | Manage events, alert on conflicts |
| Image processing | `vision`, `python`, `storage` | Analyze images, process with scripts, save results |
| Email workflow | `email`, `network`, `storage` | Read inbox, call APIs, track state |

---

## VM Sandbox

Nebo can run agent tasks inside a lightweight VM sandbox for additional isolation. This is transparent to publishers — the agent decides when to use it based on the task context. Publishers do not configure or opt into VM execution.

If your skill or agent requires specific tools or runtimes (e.g., Python packages, CLI tools), document those requirements in your SKILL.md or AGENT.md instructions so the agent can set up the environment correctly.

---

## Why Platform Capabilities?

Skills are the marketplace artifact. Platform capabilities are Nebo's infrastructure. This separation creates two important properties:

1. **Security** — Nebo controls the security boundary. Skills are readable markdown + scripts, not compiled binaries from strangers. The platform owns the permission model.

2. **Portability** — Skills don't bundle their own storage layer, their own API client, or their own file access binary. They declare what they need and the platform provides it. This prevents the "16,000 skills, 386 containing malware" problem of ecosystems where every skill ships its own plumbing.

Publishers create **knowledge and composition** — how to do bookkeeping, how to manage clients, how to repurpose content. Nebo creates the **runtime** — secure execution, platform APIs, the install experience. Neither works without the other.
