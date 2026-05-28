## Styling Rules — MANDATORY

1. ALWAYS USE TAILWINDCSS AND DAISYUI STYLES.
2. NEVER USE CUSTOM CSS — no `@layer components`, no `.nebo-*` classes, no `<style>` blocks.
3. NEVER HARDCODE COLORS — no `bg-white`, no `bg-[#F8FAFB]`, no `bg-[#E9F2F4]`, no `text-[#anything]`, no `border-[#anything]`. If you catch yourself writing a hex value or `white` in a class, STOP and use a DaisyUI semantic token instead.
4. ALWAYS USE DAISYUI SEMANTIC COLOR TOKENS — `bg-base-100`, `bg-base-200`, `bg-base-300`, `bg-primary`, `bg-secondary`, `bg-neutral`, `bg-accent`, `text-base-content`, `text-primary-content`, etc. These adapt to every theme automatically.
5. FOR SUBTLE VARIATIONS use opacity modifiers on semantic tokens — `bg-primary/10`, `text-base-content/55`, `border-base-content/30`. Never invent a custom shade.
6. STYLES ONLY GO IN `app.css` — never inline `<style>` blocks in `.svelte` files.
7. DAISYUI CONTROLS THE PALETTE, TAILWIND CONTROLS THE LAYOUT AND STATE — do not fight the theme system.
8. TEST THEME SWITCHING — if your change looks broken in dark mode, you used a hardcoded color. Fix it.
9. BORDER CONSISTENCY — if an element has a border in one state (e.g., active), give it `border border-transparent` in the other state to prevent layout shift.
10. DEPTH IS ON — `--depth: 1` is enabled. Cards, panels, and modals get DaisyUI shadow layering automatically. Do not suppress it with `shadow-none` unless the element is explicitly inset/flush.
11. ACCENT MEANS WARM — `accent` is copper/amber (`#b85c12`); `primary` is cool teal (`#0077a8`). Use `accent` for badges, highlights, and CTAs that need visual pop against the teal. Never use them interchangeably.
12. SURFACE HIERARCHY IS REAL — `base-300` borders → `base-200` sidebar/recessed → `base-100` main panels. If a UI section feels flat, it's probably using the wrong surface token. Check the hierarchy before adding custom shadows.
13. CONTRAST REQUIREMENTS — normal text must meet 4.5:1 contrast ratio against its background; large text and interactive element boundaries must meet 3:1. Never rely on color alone to convey state — always pair with a label, icon, or pattern.
14. COLOR IS NOT THE ONLY SIGNAL — status dots, error states, and active indicators must always include a label, icon, or shape change in addition to color. This is both a WCAG requirement and a design polish requirement.

## Typography Scale — MANDATORY

The entire app uses a single font (Geist). Visual hierarchy comes from SIZE + WEIGHT + OPACITY — not custom fonts or colors. Violating this scale is what makes the UI look flat and unpolished.

### The Scale

| Role | Classes | Used For |
|---|---|---|
| **Page title** | `text-base font-semibold` | Panel headers, modal titles |
| **Item title** | `text-sm font-medium` | Agent names, thread titles, card headings |
| **Body** | `text-sm` | Descriptions, message content, form labels |
| **Secondary** | `text-xs text-base-content/70` | Subtitles, role labels, preview snippets |
| **Meta / timestamp** | `text-xs text-base-content/50 font-mono` | Timestamps, message counts, durations |
| **Section label** | `text-xs font-semibold uppercase tracking-wider text-base-content/50` | Sidebar group headers, settings section dividers |
| **Code / mono** | `text-xs font-mono` | IDs, status values, tool names |

### Rules

15. NEVER USE `text-sm` FOR SECONDARY OR META CONTENT — subtitles, previews, timestamps, message counts, and durations must be `text-xs`. Using `text-sm` for everything is the #1 cause of flat, unpolished UI.
16. WEIGHT ENCODES IMPORTANCE — primary labels are `font-medium` or `font-semibold`. Supporting text is `font-normal`. Never bold secondary content.
17. OPACITY ENCODES HIERARCHY — primary content is full opacity. Secondary content is `/70`. Meta/disabled content is `/50`. Never go below `/50` for readable text (contrast floor).
18. MONO IS FOR DATA, NOT PROSE — use `font-mono` only for timestamps, counts, IDs, durations, code, and status values. All prose uses the default body font.

### Correct Examples

```svelte
<!-- Agent row in sidebar -->
<div class="text-sm font-medium">{agent.name}</div>        <!-- item title -->
<div class="text-xs text-base-content/70">{agent.role}</div> <!-- secondary -->

<!-- Thread row -->
<div class="text-sm font-medium truncate">{thread.name}</div>      <!-- item title -->
<div class="text-xs text-base-content/70 truncate">{thread.preview}</div> <!-- secondary -->
<div class="text-xs text-base-content/50 font-mono">{thread.messages} messages · {thread.updatedAt}</div> <!-- meta -->

<!-- Chat metadata -->
<span class="text-xs text-base-content/50">Nebo worked for {duration}</span>  <!-- meta -->
<span class="text-xs text-base-content/50 font-mono">Used {n} tools ↓</span>  <!-- meta -->
<span class="text-xs text-base-content/50 font-mono">{msg.time}</span>         <!-- timestamp -->

<!-- Section header -->
<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50">Agents</div>
```

## Why This Matters

DaisyUI supports multiple themes (light, dark, nord, sunset, coffee, etc.). Hardcoded colors like `bg-white` or `bg-[#F8FAFB]` render as bright white boxes on dark backgrounds. This completely breaks theme switching and is NEVER acceptable.

The `nebo` theme uses `--depth: 1` and `--noise: 0.02` — these produce the shadow layering and surface texture that make the UI feel crafted. Suppressing them makes the app look flat and unfinished.

`accent` is copper/amber — a warm color that contrasts against the cool teal `primary`. Using both intentionally gives the UI two visual gears. Using them interchangeably flattens everything back to one color.

When everything is `text-sm`, the eye has no place to land. The type scale creates visual rhythm — titles pull the eye, body delivers content, meta recedes. Without this, every screen reads as a wall of identical text.

## Svelte 5 / SvelteKit Conventions

- Use `$state()` for reactive state, `$derived()` for computed values, `$props()` for component props, `$effect()` for side effects.
- When cloning `$state` proxied objects, use `$state.snapshot()` to unwrap first: `structuredClone($state.snapshot(data))`. Never use raw `structuredClone()` on `$state` proxied arrays/objects — it throws `DataCloneError`.
- Prefer `JSON.parse(JSON.stringify(...))` only as a last resort; `$state.snapshot()` is the idiomatic approach.
- `{@const}` declarations must be immediate children of `{#if}`, `{#each}`, or `{:else}` blocks — never inside a `<div>`.

## App Routes & URLs

All routes must work on direct navigation / browser refresh.

### Agent routes (3-column layout, file-based routing)
- `/` — Redirects to `/assistant/threads`
- `/[agentId]/threads` — Threads tab, new thread empty state
- `/[agentId]/threads/[threadId]` — Threads tab, specific thread open
- `/[agentId]/runs` — Runs tab, overview with stats
- `/[agentId]/runs/[runId]` — Runs tab, specific run detail with activity timeline
- `/[agentId]/settings` — Redirects to `/[agentId]/settings/general`
- `/[agentId]/settings/[section]` — Settings tab, specific section (general, identity, persona, configure, workflows, skills, memory, permissions)

### Top-level navigation (shown in header)
- `/schedule` — Schedule (day/week/month calendar views)
- `/workspaces` — Workspaces
- `/team` — Team org chart (drag agents from sidebar to build hierarchy)
- `/marketplace` — Marketplace (has own layout)

### Marketplace (nested under `/marketplace`)
- `/marketplace` — Featured
- `/marketplace/agents` — Agent listings
- `/marketplace/agents/[id]` — Agent detail
- `/marketplace/skills` — Skill listings
- `/marketplace/skills/[id]` — Skill detail
- `/marketplace/plugins` — Plugin listings
- `/marketplace/plugins/[id]` — Plugin detail
- `/marketplace/connectors` — MCP connector listings
- `/marketplace/connectors/[id]` — MCP connector detail
- `/marketplace/categories` — Categories
- `/marketplace/collections` — Collections overview (orgs and curated bundles shared with you)
- `/marketplace/collections/[id]` — Org view or collection detail
- `/marketplace/installed` — Installed items

### Settings (nested under `/settings`, modal overlay via SettingsShell, no top nav)
- `/settings/account` — NeboAI connection
- `/settings/profile` — Profile & theme picker
- `/settings/billing` — Plan, payment, receipts
- `/settings/usage` — Plan usage & balance
- `/settings/identity` — Agent avatar/name/role
- `/settings/personality` — Presets & tuning
- `/settings/rules` — Behavior rules
- `/settings/advisors` — Advisor personas
- `/settings/agents` — Agent list & status
- `/settings/skills` — Installed skills
- `/settings/plugins` — Plugin auth status
- `/settings/mcp` — MCP server management (remote servers, OAuth/API Key/None)
- `/settings/providers` — LLM provider config (dev-only)
- `/settings/routing` — Task & lane routing (dev-only)
- `/settings/secrets` — API keys by skill (dev-only)
- `/settings/permissions` — Capabilities & auto-approval
- `/settings/sessions` — Session history
- `/settings/memories` — Memory search & layers
- `/settings/status` — System health
- `/settings/developer` — Dev mode toggle
- `/settings/about` — App info

### Other pages
- `/upgrade` — Full-screen plan selection overlay
- `/activity` — Activity/session history feed
- `/automate` — Automations list
- `/chat` — Chat page
- `/commander` — Commander org chart (read-only)
- `/events` — System events feed
- `/skills` — Installed skills list
- `/onboarding` — 5-step setup wizard (Welcome+T&C → Language → Connect → Permissions → Done)

### Panel background hierarchy

The app uses a consistent two-tone pattern. See `STYLE-GUIDE.md` for full details including hover states, selected states, and component patterns.

| Zone | Token | Examples |
|---|---|---|
| **Primary nav sidebar** | `bg-base-200` | Agent roster (col 1), schedule agent filter, marketplace subnav, team sidebar |
| **Secondary chrome / list panel** | `bg-base-200/50` | Thread list (col 2), calendar time ruler |
| **Selected row in primary nav** | `bg-base-100 border border-base-300 shadow-sm` | Card-lift pattern — lifts off bg-base-200 |
| **Selected row in list panel** | `bg-base-100 border-l-2 border-l-primary` | Active thread matches chat canvas |
| **Chat / content canvas** | `bg-base-100` | Chat area (col 3), calendar event well, settings form |
| **Canvas workspace** | `bg-base-100` | Team org chart, workspaces — cards float on it |
| **Cards on canvas** | `bg-base-200/50` + border | Org chart nodes, workspace cards — subtle tint against bg-base-100 |
| **Elevated input cards** | `bg-surface` + border + shadow-sm | Chat composer — pure white in light themes, base-100 in dark |
| **Structural borders** | `border-base-300` | Panel edges, card outlines |
| **Subtle separators** | `border-base-content/10` | Content panel headers, within-surface dividers |

### Hover states by surface

| Background | Hover class |
|---|---|
| `bg-base-200` (primary nav) | `hover:bg-base-100/70` |
| `bg-base-200/50` (secondary chrome) | `hover:bg-base-200` |
| `bg-base-100` (content panel) | `hover:bg-base-200/50` |
