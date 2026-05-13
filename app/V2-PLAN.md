# Nebo V2 — Implementation Plan

## Current State

V2 is a **fully functional UI prototype** with real navigation, state management, and mock data. The 3-column agent layout works, the marketplace has working install/uninstall with dependency cascading (including MCP connectors), the org chart supports drag-to-reparent, and settings has full feature parity with V1 (22 pages, Lucide icons, dev mode gating). TypeScript across all components. DaisyUI theme system with 11 themes. Billing matches V1's Stripe-ready structure with full-screen upgrade overlay. i18n with 25 languages ported from V1. Marketplace has been redesigned with Shopify App Store-quality detail pages, rich mock data, collections system, and conversion-focused layouts.

**What works:**
- All routes render and navigate correctly
- Marketplace: install/uninstall with cascading agent dependencies, install code redemption, back navigation on detail pages, Connectors tab for MCP servers, Collections system (create/delete personal collections, shared org collections), rich detail pages with two-column Shopify-style layout (screenshot carousel, pricing tiers, rating distribution, developer info, reviews with roles), featured page with hero banner, all listing pages with colored icon cards
- Settings: 22 pages matching V1, dev mode toggle gates Providers/Routing/Secrets, Lucide icons throughout
- Billing: unified card (plan/payment/receipts), Stripe portal link, cancel flow, full-screen upgrade overlay with plan selection
- Stores: sidebar, theme, marketplace (with auth status), devMode, toast, notifications, permissions, onboarding — all localStorage-persisted
- OrgChart: pan/zoom, drag-to-reparent, drag-from-sidebar to assign hierarchy
- Chat UX: auto-focus on typing, slash commands with scroll, @ mentions, auto-scroll, jump-to-bottom, max-width constraint, creations panel stub, file drag-and-drop with attachment thumbnails/chips, working copy/edit/redo buttons
- Agent context menu: right-click on agents for New Thread, Copy Agent ID, Settings, Delete
- Calendar: day/week/month views with agent toggles
- Command Palette: Cmd+K with 29 searchable items, arrow key navigation, grouped by category
- Notification Bell: dropdown with mark-as-read, delete, unread badge
- Toast system: success/error/info/warning with auto-dismiss
- Approval Modal: deny/approve once/approve always with permissions store, preview button in settings + onboarding
- Autonomous Mode activation: V1-parity modal (risks, disclaimer, checkbox, type ENABLE to confirm) in both settings and onboarding
- Onboarding: 5-step wizard (Welcome+T&C → Language → Connect → Permissions → Done), cohesive icon containers, text-only back/skip buttons, redirects if not complete
- Text selection: app-wide `user-select: none` with opt-in `data-selectable` for chat content, inputs, code
- Right-click: disabled globally except `data-selectable`, `data-context-menu`, inputs, code blocks
- OAuth Connect Modal: post-install auth for plugins with hasAuth (Google Workspace, Slack)
- Agent Setup Modal: 3-step wizard (Configure → Schedule → Activate) on agent install
- i18n: 25 languages via svelte-i18n, lazy-loaded, browser locale detection

**What doesn't work:**
- Zero backend integration — all mock data
- No save handlers on settings forms
- Chat composer doesn't send messages (UI complete: auto-focus, slash commands, @ mentions, auto-scroll)
- i18n strings not yet wired to components (English hardcoded, translation files present)
- Category filtering — clicking a category does nothing (no `/marketplace/categories/[slug]` route yet)

---

## Completed

- ~~Settings pages (all 22 matching V1)~~
- ~~Lucide icons replacing all emojis~~
- ~~Developer mode store gating advanced settings~~
- ~~Heartbeat removed (per-agent, not global)~~
- ~~Marketplace install model with dependency cascading~~
- ~~TypeScript across all .svelte files~~
- ~~Back navigation on marketplace detail pages~~
- ~~Install code overflow fix~~
- ~~Plugin auth status in settings~~
- ~~Billing page ported from V1 (unified card, Stripe portal, cancel flow)~~
- ~~Upgrade page as full-screen overlay (z-70, Back to Billing, Esc to close)~~
- ~~Usage page in settings nav (BarChart3 icon)~~
- ~~Status page in settings nav (Activity icon, service health, uptime)~~
- ~~MCP Settings page (add/remove/connect remote servers, 3-step modal, OAuth/API Key/None)~~
- ~~MCP Connectors marketplace tab (8 public servers, detail pages, install/uninstall)~~
- ~~Toast system (success/error/info/warning, auto-dismiss, fixed bottom-right)~~
- ~~Approval Modal (deny/approve once/approve always, permissions store)~~
- ~~Command Palette (Cmd+K, 29 items, arrow keys, grouped categories)~~
- ~~Notification Bell (dropdown, unread badge, mark-as-read, delete)~~
- ~~i18n setup (25 languages from V1, svelte-i18n, lazy-loaded)~~
- ~~Onboarding flow (4-step wizard, localStorage-persisted, root layout redirect)~~
- ~~OAuth Connect Modal (post-install auth for plugins with hasAuth)~~
- ~~Agent Setup Modal (3-step wizard: Configure → Schedule → Activate)~~
- ~~Command Palette rewrite to match V1 (CSS in app.css, slide-up animation, mouseActive, scrollIntoView, wrapping arrows, footer hints, data-selected, SVG icons)~~
- ~~Chat scroll fix (min-h-0 on column 3 + ChatPane, shrink-0 on ChatComposer)~~
- ~~Escape key closes Command Palette (handled at svelte:window level in root layout)~~
- ~~Style guide audit & fix (60+ files): typography hierarchy, opacity tiers, section labels, status dots, font-mono usage~~
- ~~Two-Tone Rule compliance: canvas pages (team, commander, workspaces) use bg-base-200, content panel headers use border-base-content/10~~
- ~~Status dot fix: online agents use bg-success (not bg-primary)~~
- ~~Column 2 selected thread uses bg-base-100 to match chat canvas~~
- ~~Settings sub-nav in Column 2: card-lift pattern with border/shadow matching sidebar rows~~
- ~~No auto-select thread on agent switch (shows New Thread empty state)~~
- ~~Agent General tab: overview, model (dev-only), skills, workflows, created date~~
- ~~Agent delete: danger zone with confirmation (editable agents only)~~
- ~~Agent editable flag: read-only badge, disabled forms, no delete for managed agents~~
- ~~Chat composer: pure white bg, paperclip icon for file attach~~
- ~~Workspaces: cards bg-base-100 on bg-base-100 canvas, sidebar hover fix, view tab hover fix~~
- ~~MCP moved out of dev-only settings into main settings group~~
- ~~MCP rewritten for remote servers (serverUrl, OAuth/API Key/None auth, registry discovery)~~
- ~~MCP add server: 3-step modal (pick → auth method → configure) matching V1 flow~~
- ~~Deleted /e, /f, /g schedule routes (redundant with /schedule)~~
- ~~Team page rewrite: agents in sidebar, only Nebo on canvas, drag-from-sidebar to assign hierarchy~~
- ~~OrgChart cards changed to bg-base-200/50 on bg-base-100 canvas~~
- ~~Chat auto-focus: global keydown listener focuses composer when user starts typing~~
- ~~Slash command arrow-key scroll-into-view~~
- ~~@ mention agent addressing in chat composer (detect @, filter agents, arrow nav, Tab/Enter select)~~
- ~~Chat auto-scroll + jump-to-bottom button (matching V1 scroll state machine)~~
- ~~adapter-static SPA deployment (fallback: index.html, ssr: false)~~
- ~~Chat max-width constraint (max-w-3xl centered) + Creations panel stub (right side split)~~
- ~~bg-surface token: pure white elevated cards in light themes, base-100 fallback in dark themes~~
- ~~File drag-and-drop on chat: attachment UI with image thumbnails + file chips, composer-level + thread-level drop zones~~
- ~~Chat copy/edit/redo: clipboard copy with checkmark feedback, inline edit box for user messages, redo button for assistant messages~~
- ~~Agent context menu: right-click on sidebar agents (New Thread, Copy Agent ID, Settings, Delete for editable)~~
- ~~Text selection control: app-wide user-select none, opt-in via data-selectable attribute~~
- ~~Right-click control: browser context menu blocked except data-selectable, data-context-menu, inputs, code~~
- ~~Onboarding rewrite: 5-step flow with T&C acceptance, 25-language selector, cohesive icon containers, text-only secondary buttons~~
- ~~Autonomous Mode activation modal: V1 parity (risks box, disclaimer, checkbox, type ENABLE, red confirm button)~~
- ~~Settings permissions V1 parity: vertical list with dividers, autonomous activation modal, approval dialog preview~~
- ~~Marketplace redesign: Featured page with hero banner, colored icon avatars, curated sections (Popular, Top Agents, Essential Skills, Plugins, Browse by Category, Collections, Build CTA)~~
- ~~Marketplace listing pages: agents, skills, plugins redesigned with colored icon cards, filled star ratings, install status~~
- ~~Marketplace detail pages: two-column Shopify-style layout for agents, skills, plugins — left sidebar (icon, name, rating, price, install button, developer info) + right main content (screenshot carousel with chevron nav, video placeholder, about + feature checklist, works-with badges, requirements, pricing tier cards with "Popular" badge, rating distribution bar chart, individual review cards with role/duration)~~
- ~~Marketplace Collections: renamed from "Private", flat nav item in sidebar, collections overview page with create modal (name + desc + item picker with search), personal collections with delete, shared org collections, collection detail pages~~
- ~~Rich marketplace mock data: features arrays, pricing tiers (Starter/Team/Enterprise), rating distributions, developer info (website/support/launch date), screenshots as objects, reviews with role + duration, worksWith integrations~~
- ~~Marketplace category emojis removed from sidebar and featured page (cleaner professional look)~~
- ~~NeboLabs → NeboLoop branding fix across all mock data (author, developer, website, support email)~~
- ~~Output panel renamed to Creations panel (all labels, variables, comments, docs)~~
- ~~Idiomatic SvelteKit file-based routing: decomposed 979-line monolith into nested routes with layouts, setContext for agent data, URL-driven tab/section state, deep links for threads/runs/settings~~

---

## P0 — ~~Critical Missing Features~~ ALL DONE

All P0 items are complete:
- ~~Command Palette (Cmd+K)~~ — `CommandPalette.svelte`, wired to header search + Cmd+K
- ~~Onboarding Flow~~ — `/onboarding` 4-step wizard, `onboarding.ts` store, root layout redirect
- ~~OAuth Connect Modal~~ — `OAuthConnectModal.svelte`, triggered on plugin install for hasAuth plugins
- ~~Agent Setup Modal~~ — `AgentSetupModal.svelte`, triggered on agent install
- ~~Notification System~~ — `NotificationBell.svelte` + `notifications.ts` store, wired in header
- ~~Approval Modal~~ — `ApprovalModal.svelte` + `permissions.ts` store
- ~~Toast System~~ — `Toast.svelte` + `toast.ts` store, rendered at root layout
- ~~i18n~~ — 25 languages via svelte-i18n, lazy-loaded

---

## P1 — Important Feature Gaps

### ~~8. Marketplace Search~~ DONE

Real search input with 150ms debounce, filters across skills/agents/plugins/connectors by name, description, and category. Results shown in dropdown (max 8), clicking navigates to detail page. Type badge on each result.

---

### 9. Category Filtering

Clicking a category card does nothing.

**What to build:**
- Route: `/marketplace/categories/[slug]`
- Shows all items matching that category
- Back link to categories list

**Files to create:**
- `src/routes/marketplace/categories/[slug]/+page.svelte` (new)

**Files to modify:**
- `src/routes/marketplace/categories/+page.svelte` (link cards to `/marketplace/categories/{slug}`)

---

### 10. Settings Save Handlers

All settings forms are display-only. State resets on navigation.

**What to build:**
- Settings store persisted to localStorage
- Save button or auto-save on each page
- Toast feedback on save
- Pages: Profile, Identity, Personality, Permissions, Rules

**Files to create:**
- `src/lib/stores/settings.ts` (new)

---

### 11. Chat Message Sending

Chat composer has send button but doesn't actually add messages.

**What to build:**
- Chat store per thread
- `onsend` pushes user message to store
- Simulated agent response after 1-2s delay
- Messages persist within session

**Files to create:**
- `src/lib/stores/chat.ts` (new)

**Files to modify:**
- `src/routes/+page.svelte` (wire onsend)
- `src/lib/components/chat/ChatPane.svelte` (read from store)

---

## P2 — Polish & Enhancement

### 12. Agent Detail Improvements

- Workflow editor modal save logic
- ~~Agent delete confirmation dialog~~ (done — General tab, editable agents only)
- Agent pause/resume toggle
- Activity tab: session history instead of run count

### 13. Review System for Marketplace

- "Write a review" button on detail pages
- Modal: star rating picker + textarea + submit
- New review added in memory

---

## P3 — Future / Nice-to-Have

### ~~14. Schedule route cleanup~~
Deleted `/e`, `/f`, `/g` — redundant. `/schedule` handles day/week/month views with built-in switcher.

### 15. Workspace apps
Build out at least one functional workspace view (CRM or Analytics).

### 16. Real NeboLoop integration
Replace mock data with live API calls to NeboLoop MCP server.

### 17. Keyboard shortcuts
`Cmd+N` (new thread), `Cmd+,` (settings), `Cmd+1-8` (switch agent).

### 18. Responsive / mobile layout
Add responsive breakpoints for tablet and mobile.

### 19. Drag-and-drop agent reordering
Reorder agents in the left sidebar via drag-and-drop.

### 20. Marketplace "You might also like"
Related items section on detail pages.

---

## File Inventory

### Routes (50+ total)
```
/                               Redirect → /assistant/threads
/[agentId]                      Redirect → /[agentId]/threads
/[agentId]/threads              Threads tab, new thread empty state (functional)
/[agentId]/threads/[threadId]   Threads tab, specific thread (functional)
/[agentId]/runs                 Runs tab with stats (functional)
/[agentId]/settings             Redirect → /[agentId]/settings/general
/[agentId]/settings/[section]   Settings section (general/identity/persona/configure/workflows/skills/memory/permissions)
/activity                       Session history feed (display)
/automate                       Automations list with toggles (display)
/chat                           Chat page (display + composer UI)
/commander                      Org chart (read-only)
# /e, /f, /g removed — schedule views handled by /schedule
/events                         System events feed with filter
/onboarding                     5-step setup wizard (functional)
/marketplace                    Featured page (functional)
/marketplace/agents             Agent catalog (functional)
/marketplace/agents/[id]        Agent detail — two-column layout (functional)
/marketplace/categories         Category grid (display)
/marketplace/connectors         MCP connector catalog (functional)
/marketplace/connectors/[id]    MCP connector detail (functional)
/marketplace/collections        Collections overview — create/browse (functional)
/marketplace/collections/[id]   Collection/org detail with items (functional)
/marketplace/installed          Installed list (functional)
/marketplace/plugins            Plugin catalog (functional)
/marketplace/plugins/[id]       Plugin detail — two-column layout (functional)
/marketplace/skills             Skill catalog (functional)
/marketplace/skills/[id]        Skill detail — two-column layout (functional)
/schedule                       Calendar shell
/settings/account               NeboLoop connection (display)
/settings/profile               Theme picker + profile fields (theme works)
/settings/billing               Plan + payment + receipts (functional, Stripe-ready)
/settings/usage                 Plan usage + balance (display)
/settings/identity              Agent avatar/name/role (display)
/settings/personality           Presets + tuning dimensions (display)
/settings/rules                 Behavior rules with toggles (display)
/settings/advisors              Advisor personas (display)
/settings/agents                Agent list with status (display)
/settings/skills                Installed skills with toggles (display)
/settings/plugins               Plugin auth status + connect/disconnect (functional)
/settings/providers             LLM provider config (display, dev-only)
/settings/routing               Task + lane routing (display, dev-only)
/settings/secrets               API keys by skill (display, dev-only)
/settings/mcp                   MCP server management (functional, remote servers)
/settings/permissions           Capabilities + auto-approval + autonomous modal (functional)
/settings/sessions              Session history + cleanup (display)
/settings/memories              Memory search + layer filter (functional)
/settings/status                System health + service status (display)
/settings/developer             Dev mode toggle + sideloading (functional)
/settings/about                 App info + resources (display)
/skills                         Installed skills list (display)
/team                           Org chart with drag-to-reparent (functional)
/upgrade                        Full-screen plan selection overlay (functional)
/workspaces                     Workspace apps (display)
```

### Components (24 total)
```
AgentTabBar.svelte              Agent header + Threads/Runs/Settings tab links (shared across tabs)
AgentSetupModal.svelte          3-step agent config wizard (Configure → Schedule → Activate)
ApprovalModal.svelte            Trust & safety approval (deny/approve once/approve always)
Avatar.svelte                   Agent/user avatar display
ColorCalendarShell.svelte       Calendar view wrapper
ColorDayView.svelte             Day calendar grid
ColorMonthView.svelte           Month calendar grid
ColorWeekView.svelte            Week calendar grid
CommandPalette.svelte           Cmd+K search overlay (29 items, arrow keys, categories)
DayDetailPane.svelte            Schedule day details sidebar
MarketplaceShell.svelte         Marketplace layout (unused, using +layout)
MiniMonth.svelte                Date picker mini calendar
NeboShell.svelte                Main app layout shell
NotificationBell.svelte         Bell icon + dropdown (mark-as-read, delete, unread badge)
OAuthConnectModal.svelte        Post-install OAuth auth prompt for plugins
OrgChart.svelte                 Interactive org chart with drag-to-reparent
SettingsShell.svelte            Settings modal with Lucide icons + dev gating
Sidebar.svelte                  Global left navigation
StatusDot.svelte                Agent status indicator
Toast.svelte                    Fixed bottom-right toast (success/error/info/warning)
UserMenu.svelte                 User avatar/menu in sidebar footer
chat/ChatComposer.svelte        Message input + file attach + slash commands
chat/ChatPane.svelte            Message display with tool results
chat/SlashCommandMenu.svelte    Slash command dropdown
```

### Stores (9 total)
```
collections.ts    Collections CRUD — create/delete personal, browse shared org collections
devmode.ts        Developer mode toggle (localStorage-persisted, gates settings nav)
marketplace.ts    Install/uninstall with cascading dependencies + connector support
notifications.ts  Notification list with mock data, mark-as-read, unread count
onboarding.ts     Onboarding completion state (localStorage-persisted)
permissions.ts    Auto-approved actions (localStorage-persisted)
sidebar.ts        Sidebar collapsed state
theme.ts          Theme selection (localStorage-persisted)
toast.ts          Toast queue with addToast/removeToast, auto-dismiss
```

---

## Recommended Build Order (remaining)

1. **Category Filtering** (#9) — quick win
2. **Chat Sending** (#11) — makes chat page real (UI already complete)
3. **Settings Save** (#10) — makes settings pages real
4. **i18n wiring** — replace hardcoded English with `$t()` calls across all components
5. **P2 items** (#12-13) in any order
6. **P3 items** (#15-20) as time permits
