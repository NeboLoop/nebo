# ADR-004: Promote App Store to Top Navigation

| Field       | Value                    |
|-------------|--------------------------|
| **Status**  | Proposed                 |
| **Date**    | 2026-02-07               |
| **Author**  | Nebo (on behalf of user) |
| **Depends** | —                        |

---

## Context

The App Store currently lives as a tab inside **Settings → Plugins** (`/settings/plugins`). To reach it, users must:

1. Click the **Settings cog** in the top nav
2. Navigate to the **Plugins** section in the settings sidebar
3. Switch to the **App Store** tab

This buries one of the most important discovery surfaces behind two levels of navigation. The App Store is how users find and install new capabilities — it deserves first-class placement.

### Current Navigation Layout

```
┌──────────────────────────────────────────────────────────────────┐
│  [Nebo Logo]   Chat   Schedule                     [⚙ Settings] │
└──────────────────────────────────────────────────────────────────┘
```

The top nav (`AppNav.svelte`) has:
- **Left:** Logo + nav links (Chat, Schedule)
- **Right:** Settings cog icon (links to `/settings`)

The App Store is only accessible via `Settings → Plugins → App Store tab`.

---

## Decision

**Add an App Store button to the top navigation bar**, positioned immediately to the left of the Settings cog. This gives the App Store the same top-level visibility as Chat and Schedule, without requiring users to dig through Settings.

### New Navigation Layout

```
┌──────────────────────────────────────────────────────────────────┐
│  [Nebo Logo]   Chat   Schedule              [🏪 Store] [⚙]     │
└──────────────────────────────────────────────────────────────────┘
```

### New Route

Create a dedicated top-level route for the App Store at `/store`. This separates the App Store browsing experience from plugin configuration.

```
app/src/routes/(app)/store/+page.svelte    → App Store (browse, search, install)
app/src/routes/(app)/settings/plugins/     → Plugin management (configure, toggle, settings)
```

---

## Implementation

### Phase 1: Add Store Button to AppNav

**File: `app/src/lib/components/navigation/AppNav.svelte`**

Add a Store icon button next to the Settings cog in the right-side group:

```svelte
<!-- Right side: Store + Settings -->
<div class="hidden sm:flex items-center gap-1">
    <!-- App Store -->
    <a
        href="/store"
        class="flex items-center justify-center w-9 h-9 rounded-lg text-base-content/50 
               hover:text-base-content hover:bg-base-200 transition-colors"
        class:active-icon={currentPath.startsWith('/store')}
        aria-label="App Store"
    >
        <!-- Store/grid icon (lucide: LayoutGrid or Store) -->
        <svg class="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
            <rect x="3" y="3" width="7" height="7" rx="1" />
            <rect x="14" y="3" width="7" height="7" rx="1" />
            <rect x="3" y="14" width="7" height="7" rx="1" />
            <rect x="14" y="14" width="7" height="7" rx="1" />
        </svg>
    </a>
    
    <!-- Settings -->
    <a href="/settings" ...existing cog...>
    </a>
</div>
```

The Store icon should highlight (using the same `active-icon` treatment) when the user is on `/store`.

**Mobile menu:** Add the Store link in the mobile dropdown as well, positioned before Settings in the bottom section.

### Phase 2: Create `/store` Route

**File: `app/src/routes/(app)/store/+page.svelte`**

Extract the App Store tab content from `/settings/plugins` into its own page. This page gets:

- **Full-width layout** (not constrained by the settings sidebar)
- **Hero/header section** with search prominently featured
- **Two sub-tabs:** Apps and Skills (same as current)
- **Grid layout** for browsing (same card format)
- **Category sidebar or filter chips** for discovery

The page reuses the same API calls (`listStoreApps`, `listStoreSkills`, `installStoreApp`, etc.) — just relocated.

### Phase 3: Simplify Settings → Plugins

After the Store is promoted, the `/settings/plugins` page simplifies to **installed plugin management only**:

- Remove the "Installed / App Store" tab switcher
- Remove all store browsing UI from the plugins page
- Keep: plugin list, toggle, settings configuration, connection status
- Add a small link: "Browse more in the App Store →" that links to `/store`

### Phase 4: Mobile Menu Update

Update the mobile menu in `AppNav.svelte` to include the Store link:

```svelte
<div class="border-t border-base-300 pt-4 space-y-1">
    <a href="/store" class="nav-link" class:active={currentPath.startsWith('/store')} onclick={closeMobileMenu}>
        <!-- store icon -->
        App Store
    </a>
    <a href="/settings" class="nav-link" class:active={currentPath.startsWith('/settings')} onclick={closeMobileMenu}>
        <!-- settings icon -->
        Settings
    </a>
</div>
```

---

## File Changes

| File | Change |
|------|--------|
| `app/src/lib/components/navigation/AppNav.svelte` | Add Store icon button next to Settings cog (desktop + mobile) |
| `app/src/routes/(app)/store/+page.svelte` | **New** — dedicated App Store page (extracted from plugins) |
| `app/src/routes/(app)/settings/plugins/+page.svelte` | Remove App Store tab; keep installed-only management; add "Browse Store →" link |
| `app/src/routes/(app)/settings/+layout.svelte` | Rename "Plugins" sidebar label to "Installed Plugins" (optional, for clarity) |
| `app/static/app.css` | Add `.active-icon` style for highlighted nav icons |

---

## Design Details

### Store Icon

Use a **grid/blocks icon** (4 squares) to differentiate from the Settings cog. This is a common pattern for app stores/launchers (macOS Launchpad, iOS App Store grid). Alternatives considered:

| Icon | Pros | Cons |
|------|------|------|
| Grid (4 squares) ✅ | Universally recognized as "apps" | Could be confused with dashboard |
| Shopping bag | Clear "store" metaphor | Implies commerce/money |
| Puzzle piece | Already used for Plugins in settings | Would be confusing to reuse |
| Plus/circle | Suggests "add new" | Too generic |

**Recommendation:** Grid icon (4 squares) with `aria-label="App Store"` for accessibility.

### Active State

When the user is on `/store`, the Store icon should be highlighted the same way the Settings cog would be when on `/settings`:

```css
/* app.css */
.active-icon {
    color: var(--primary);
    background-color: oklch(from var(--primary) l c h / 0.1);
}
```

### Tooltip

Add a `title="App Store"` attribute for hover tooltip on desktop. No tooltip needed on mobile.

### Badge (Future)

Reserve space for a notification badge on the Store icon to indicate:
- New apps/skills available
- Updates available for installed plugins

This is **not in scope** for the initial implementation but the icon container should accommodate it.

---

## UX Flow Comparison

### Before (Current)

```
User wants to install a new skill
  → Clicks Settings cog
  → Scrolls to find "Plugins" in sidebar
  → Clicks "Plugins"
  → Clicks "App Store" tab
  → Searches/browses
  → Installs

Clicks: 4 minimum
Time: ~8 seconds
```

### After (Proposed)

```
User wants to install a new skill
  → Clicks Store icon in top nav
  → Searches/browses
  → Installs

Clicks: 2 minimum
Time: ~3 seconds
```

---

## Alternatives Considered

### 1. Add "Store" as a full nav link (like Chat, Schedule)

```
[Nebo Logo]   Chat   Schedule   Store                     [⚙]
```

**Rejected:** The Store is not a daily-use destination like Chat. Making it a full nav link gives it too much weight and clutters the left side. An icon button on the right keeps it accessible without competing with primary navigation.

### 2. Keep Store in Settings, add a shortcut

Add a keyboard shortcut or command palette entry to jump to the store.

**Rejected:** Doesn't solve the discoverability problem. New users won't know the shortcut exists.

### 3. Add Store to the Settings sidebar as a top-level section

Make "App Store" its own section in the settings sidebar instead of a tab within Plugins.

**Rejected:** Still requires navigating to Settings first. Doesn't solve the core problem of the Store being buried.

### 4. Dropdown menu from Store icon

Click the Store icon → shows a dropdown with recent/featured apps, with "See all" linking to the full store page.

**Deferred:** Good idea for Phase 2 but adds complexity. Start with a simple link first.

---

## Migration

This is a **non-breaking, additive change**:

1. The new `/store` route is brand new — no existing links break
2. `/settings/plugins` continues to work with a simplified scope
3. Any deep links to `/settings/plugins` with the store tab active should redirect to `/store` (add a query param check: `?tab=store` → redirect to `/store`)
4. No backend changes required — same API endpoints

---

## Risks

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| Users confused by Store vs Plugins pages | Low | Clear labeling: Store = browse/install, Plugins = configure installed |
| Top nav feels crowded with two icon buttons | Low | Icons are compact (w-9 h-9); plenty of space between nav links and icons |
| Store page duplicates code from plugins page | Medium | Extract shared components (StoreAppCard, StoreSkillCard) into `$lib/components/store/` |

---

## Success Metrics

- **Discoverability:** Users find and click the Store icon without guidance
- **Reduced friction:** Time-to-install for a new app/skill drops from ~8s to ~3s
- **Clean separation:** Plugin configuration stays focused; Store browsing gets room to grow
