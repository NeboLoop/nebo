# App UX Restructure — SME Reference

## ICP
Non-technical people. The bot's audience is everyday users, not developers.
Power users/developers must still have deep access, but it should never be in the way.

## Design Philosophy
Apple System Settings pattern:
- Flat list, no group headers — whitespace gaps imply grouping
- Profile card at top
- Ordered by importance (most-used first, technical last)
- Everything visible, nothing hidden — just good ordering and spacing
- No collapsible "Advanced" toggle

## Design System
- **Single source of truth:** `app/src/app.css` — copied from neboai's app.css with desktop-only additions appended
- **Stack:** Tailwind CSS v4.1 + DaisyUI v5.5 + SvelteKit
- **Fonts:** DM Sans (body), Satoshi (display)
- **Palette:** Teal (#14b8a6 primary), Indigo (#6366f1 secondary), Amber (#f59e0b tertiary)
- **Dark mode:** Automatic via `prefers-color-scheme` with OKLCH overrides for Apple-level contrast
- **Header height:** Desktop = 64px (neboai web = 72px)

## Top Nav (Implemented)
```
Chat | [Store puzzle-piece icon] | [Settings gear icon]
```
- Chat is the only text nav link (default items prop)
- Store and Settings are icon-only buttons on the right side
- Mobile: hamburger expands to show all links as text

### File
`app/src/lib/components/navigation/AppNav.svelte`

## Store / Marketplace (Implemented)

### Route: `/store`
- `/store` — featured agents, skills, workflows, editorial sections, "Build for Nebo" CTA
- `/store/agents` — browse all agents
- `/store/skills` — browse all skills
- `/store/workflows` — browse all workflows
- `/store/categories` — browse by category

### Key Components
- `app/src/lib/components/marketplace/LargeCard.svelte` — agent cards (2-col grid)
- `app/src/lib/components/marketplace/ListCard.svelte` — list items in horizontal grids
- `app/src/lib/components/MarketplaceGrid.svelte` — Apple App Store-style horizontal paging (3 rows, snap scroll)
- `app/src/lib/components/marketplace/sections/SectionEditorial.svelte` — featured carousel
- `app/src/lib/components/marketplace/sections/SectionListGrid.svelte` — titled grid section
- `app/src/lib/components/marketplace/sections/SectionTopRanked.svelte` — ranked grid with numbers
- `app/src/lib/components/InstallCode.svelte` — code redemption (SKILL-XXXX, AGNT-XXXX, etc.)

### Data Flow
- Products: `GET /api/v1/store/products?type={agent|skill|workflow}`
- Featured: `GET /api/v1/store/featured?type={agent|skill|workflow}`
- Types: `$lib/types/marketplace.ts` — `AppItem`, `toAppItem()`

## Current Settings Sidebar (7 groups, group headers)
```
EXTEND: NeboAI, Apps, Skills, Integrations
YOU: Profile
CHARACTER: Identity, Soul, Rules, Notes
MIND: Routing, Providers, Memories, Advisors
BEHAVIOR: Heartbeat, Permissions
SYSTEM: Sessions, Status
DEVELOPER: Developer
```

### File
`app/src/routes/settings/+layout.svelte`

### All Setting Routes (on disk)
```
about, account, advisors, agents, billing, browser, developer,
events, identity, mcp, permissions, personality, plugins, profile,
providers, routing, rules, secrets, skills, status, updates, usage
```
Note: The settings route set is filesystem-driven under `app/src/routes/settings`.

## Planned Settings Sidebar (Apple-style) — NOT YET IMPLEMENTED
```
[Profile card — avatar + name + NeboAI status]

Personality
Rules
Permissions

NeboAI
Models

  (whitespace gap — below here is power-user territory)

Identity
Memories
Heartbeat
Routing
Advisors
Integrations
Notes

  (whitespace gap — system/dev)

Sessions
Status
Developer
```

### Remaining Work
- [ ] Remove group headers, switch to flat list with whitespace gaps
- [ ] Add profile card at top of sidebar
- [ ] Rename "Soul" → "Personality" in sidebar label (route already exists at `/settings/personality`)
- [ ] Rename "Providers" → "Models" in sidebar label
- [ ] Remove Apps and Skills from settings sidebar (already in Store)
- [ ] Reorder items per planned layout

## Settings Page Audit

### ESSENTIAL (agent won't work without)
| Page | Route | What It Does | Who |
|------|-------|-------------|-----|
| Profile | /settings/profile | User's name, location, timezone, occupation, interests, goals, communication style | Everyone |
| Personality | /settings/personality | System prompt + tuning sliders (voice, length, emoji, formality, proactivity) | Everyone |
| Rules | /settings/rules | Safety guardrails + custom behavioral rules | Everyone |
| Providers | /settings/providers | AI provider API keys, Janus models, model enable/disable | Everyone |
| Permissions | /settings/permissions | Autonomous mode toggle, capability toggles, tool approval policy | Everyone |
| NeboAI | /settings/neboai | OAuth connection to cloud, account + bot status, Janus usage | Everyone |

### SECONDARY (power user / nice-to-have)
| Page | Route | What It Does | Who |
|------|-------|-------------|-----|
| Identity | /settings/identity | Agent name, avatar, creature archetype, role, vibe, emoji | Everyone |
| Memories | /settings/memories | Browse/search/edit agent's learned facts (tacit, daily, entity) | Power users |
| Heartbeat | /settings/heartbeat | Proactive background task schedule + markdown task list | Power users |
| Routing | /settings/routing | Model assignment per task type + custom aliases + lane routing | Power users |
| Advisors | /settings/advisors | Internal deliberation sub-agents with roles | Power users |
| Sessions | /settings/sessions | Conversation history browser | Everyone |

### DEVELOPER-ONLY
| Page | Route | What It Does | Who |
|------|-------|-------------|-----|
| Integrations | /settings/integrations | Connect MCP servers (URL + auth type + name) | Developers |
| Notes | /settings/notes | System environment info + custom context notes for tools | Developers |
| Status | /settings/status | Real-time system health: MCP, DB, WS, uptime, agents | Developers |
| Developer | /settings/developer | Dev tools (delegated to DeveloperSection.svelte) | Developers |

## App Navigation Architecture

### File Locations
- Top nav: `app/src/lib/components/navigation/AppNav.svelte`
- App layout: `app/src/routes/+layout.svelte`
- Agent layout: `app/src/routes/[agentId]/+layout.svelte`
- Sidebar: `app/src/lib/components/sidebar/Sidebar.svelte`
- Settings layout: `app/src/routes/settings/+layout.svelte`
- Marketplace layout: `app/src/routes/marketplace/+layout.svelte`
- Marketplace home: `app/src/routes/marketplace/+page.svelte`
- All settings pages: `app/src/routes/settings/{section}/+page.svelte`

### Layout Modes
1. **Full-height** (chat): `h-dvh flex flex-col overflow-hidden` — sidebar + chat
2. **Normal** (everything else): padded `p-6`, max-w-1400px, no sidebar

### Sidebar (Chat only, 240px)
- "My Chat" button (default conversation)
- Loops section (expandable, shows channels + agents within loops)
- Standalone Agents (agents not linked to a Loop)
- Activity indicators: Heartbeat, Events, Desktop (pulse dots)

### Styling
- All styles in `app/src/app.css` (unified with neboai design system)
- Nav links: `flex gap-2 px-3 py-2 rounded-lg text-sm`
- Active: `text-primary bg-primary/10`
- Sidebar items: same pattern
- Section labels: `text-xs uppercase tracking-wider text-base-content/40`

## Key User Flows (Priority Order)
1. **Onboarding** → NeboAI OAuth (happens during setup)
2. **Install an agent** → Store → gives bot a job
3. **Install a skill** → Store → gives bot capability
4. **Chat remotely** → Loop channels → shows online in sidebar
5. **Use own LLMs** → Settings > Providers → power user
6. **Customize personality** → Settings > Personality
7. **Set permissions** → Settings > Permissions

## Component Reuse Notes
- `RuleSection.svelte` — used for both Rules AND Notes (generic section/item UI)
- `ProvidersSection.svelte` — full providers UI (will be renamed to Models)
- `SkillEditorModal.svelte` — skill create/edit
- `AppDetailModal.svelte` — app details
- Auto-save pattern: 500ms debounce with status indicator (Rules, Notes)
- Most other pages: save-on-submit with loading spinner

## Open Issues
- Settings sidebar still uses old grouped structure — needs Apple-style flat list rework
- "Soul" label in sidebar needs rename to "Personality"
- "Providers" label needs rename to "Models"
- Apps and Skills still in settings sidebar — should be removed (moved to Store)
- Workflows and Events removed from top nav — need to decide where they go
  - Workflows may become part of Store (install workflow templates)
  - Events may become a panel within chat or a sidebar section
- Profile page has commented-out theme/appearance controls (TODO)
