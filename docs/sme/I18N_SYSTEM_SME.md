# I18N System SME

Subject Matter Expert reference for Nebo's internationalization (i18n) system.

Last updated: 2026-05-15

---

## Table of Contents

1. [Architecture Overview](#architecture-overview)
2. [Library and Dependencies](#library-and-dependencies)
3. [Supported Locales](#supported-locales)
4. [Locale File Structure](#locale-file-structure)
5. [Translation Key Naming Patterns](#translation-key-naming-patterns)
6. [Translation Loading Strategy](#translation-loading-strategy)
7. [Locale Detection and Selection](#locale-detection-and-selection)
8. [Runtime Locale Switching](#runtime-locale-switching)
9. [Using Translations in Components](#using-translations-in-components)
10. [Variable Interpolation](#variable-interpolation)
11. [Pluralization](#pluralization)
12. [Date, Time, and Number Formatting](#date-time-and-number-formatting)
13. [RTL Language Support](#rtl-language-support)
14. [Missing Translation Fallback](#missing-translation-fallback)
15. [Key Coverage Analysis](#key-coverage-analysis)
16. [Backend Language Persistence](#backend-language-persistence)
17. [Cross-System Interactions](#cross-system-interactions)
18. [Adding a New Locale](#adding-a-new-locale)
19. [Known Gaps and Migration Status](#known-gaps-and-migration-status)
20. [Key Files Reference](#key-files-reference)

---

## Architecture Overview

```
+------------------------------------------------------------------+
|                        Root Layout                                |
|  app/src/routes/+layout.svelte                                   |
|  import '$lib/i18n'  <-- side-effect import triggers init()      |
+------------------------------------------------------------------+
         |
         v
+------------------------------------------------------------------+
|                    i18n Module (index.ts)                         |
|  app/src/lib/i18n/index.ts                                       |
|                                                                  |
|  1. register() -- lazy-load each locale via dynamic import()     |
|  2. detectLocale() -- localStorage > navigator.language > 'en'   |
|  3. init({ fallbackLocale: 'en', initialLocale: detected })      |
+------------------------------------------------------------------+
         |
         v
+------------------------------------------------------------------+
|                    svelte-i18n ^4.0.1                             |
|                                                                  |
|  Exports used by Nebo:                                           |
|    - init()       -- configure library                           |
|    - register()   -- register locale loaders                     |
|    - t            -- readable store, translation function         |
|    - locale       -- writable store, current locale string        |
+------------------------------------------------------------------+
         |                          |
         v                          v
+-------------------+    +----------------------------+
| Svelte Components |    |     Locale JSON Files      |
|                   |    |  app/src/lib/i18n/locales/  |
| import { t }      |    |                            |
| from 'svelte-i18n'|    |  en.json   (1409 lines)    |
|                   |    |  de.json   (1370 lines)    |
| {$t('key')}       |    |  ar.json   (1370 lines)    |
| {$t('key',        |    |  ja.json   (1370 lines)    |
|   { values: {} })}|    |  ... 25 total files        |
+-------------------+    +----------------------------+
         |
         v
+------------------------------------------------------------------+
|                   Persistence Layer                               |
|                                                                  |
|  localStorage['nebo_locale']  -- client-side preference          |
|  PUT /api/v1/user/me/preferences { language: 'xx' }             |
|    -- backend persists to SQLite user_preferences table          |
+------------------------------------------------------------------+
```

### Data Flow: Locale Selection

```
App Boot
  |
  +---> detectLocale()
  |       |
  |       +---> Check localStorage['nebo_locale']
  |       |       |
  |       |       +-- Found? --> Use saved locale
  |       |       +-- Not found? --> Fall through
  |       |
  |       +---> Check navigator.language
  |       |       |
  |       |       +-- Exact match in supportedLocales? --> Use it
  |       |       +-- Base language match (e.g. 'fr' from 'fr-CA')? --> Use it
  |       |       +-- No match? --> Fall back to 'en'
  |       |
  |       +---> SSR (non-browser)? --> Always 'en'
  |
  +---> init({ fallbackLocale: 'en', initialLocale: detected })
  |
  +---> First render triggers lazy load of the chosen locale JSON
  |
  +---> $t('key') resolves from loaded locale, falls back to 'en'
```

---

## Library and Dependencies

| Dependency   | Version  | Purpose                                    |
|-------------|----------|--------------------------------------------|
| `svelte-i18n` | `^4.0.1` | Core i18n library for Svelte               |

svelte-i18n is built on top of the `intl-messageformat` package (part of FormatJS), which
implements ICU MessageFormat for variable interpolation, pluralization, and select expressions.

The library provides:
- **`init()`** -- configure fallback locale, initial locale, and loading behavior
- **`register()`** -- register a locale with a lazy-loading function
- **`t`** -- a Svelte readable store that returns the translation function
- **`locale`** -- a Svelte writable store holding the current locale string
- **`isLoading`** -- a readable store (boolean) indicating if a locale is being loaded
- **`waitLocale()`** -- returns a promise that resolves when the current locale is loaded

Nebo uses `t`, `locale`, `init`, and `register`. It does **not** currently use the built-in
`$date`, `$number`, or `$time` formatters from svelte-i18n.

---

## Supported Locales

25 locales are registered and supported. Each has a dedicated JSON file under
`app/src/lib/i18n/locales/`.

| # | Code    | Language              | Script   | Direction | File          |
|---|---------|----------------------|----------|-----------|---------------|
| 1 | `en`    | English              | Latin    | LTR       | `en.json`     |
| 2 | `de`    | German (Deutsch)     | Latin    | LTR       | `de.json`     |
| 3 | `es`    | Spanish (Espanol)    | Latin    | LTR       | `es.json`     |
| 4 | `fr`    | French (Francais)    | Latin    | LTR       | `fr.json`     |
| 5 | `it`    | Italian (Italiano)   | Latin    | LTR       | `it.json`     |
| 6 | `pt`    | Portuguese           | Latin    | LTR       | `pt.json`     |
| 7 | `pt-BR` | Portuguese (Brazil)  | Latin    | LTR       | `pt-BR.json`  |
| 8 | `nl`    | Dutch (Nederlands)   | Latin    | LTR       | `nl.json`     |
| 9 | `sv`    | Swedish (Svenska)    | Latin    | LTR       | `sv.json`     |
| 10| `pl`    | Polish (Polski)      | Latin    | LTR       | `pl.json`     |
| 11| `tr`    | Turkish (Turkce)     | Latin    | LTR       | `tr.json`     |
| 12| `ru`    | Russian              | Cyrillic | LTR       | `ru.json`     |
| 13| `uk`    | Ukrainian            | Cyrillic | LTR       | `uk.json`     |
| 14| `ar`    | Arabic               | Arabic   | **RTL**   | `ar.json`     |
| 15| `he`    | Hebrew               | Hebrew   | **RTL**   | `he.json`     |
| 16| `hi`    | Hindi                | Devanagari | LTR     | `hi.json`     |
| 17| `bn`    | Bengali (Bangla)     | Bengali  | LTR       | `bn.json`     |
| 18| `th`    | Thai                 | Thai     | LTR       | `th.json`     |
| 19| `vi`    | Vietnamese           | Latin    | LTR       | `vi.json`     |
| 20| `id`    | Indonesian           | Latin    | LTR       | `id.json`     |
| 21| `ms`    | Malay                | Latin    | LTR       | `ms.json`     |
| 22| `ja`    | Japanese             | CJK      | LTR       | `ja.json`     |
| 23| `ko`    | Korean               | Hangul   | LTR       | `ko.json`     |
| 24| `zh-CN` | Chinese (Simplified) | CJK      | LTR       | `zh-CN.json`  |
| 25| `zh-TW` | Chinese (Traditional)| CJK      | LTR       | `zh-TW.json`  |

The `supportedLocales` array in `index.ts` is the canonical list:
```typescript
const supportedLocales = [
  'en', 'de', 'es', 'pt-BR', 'zh-CN', 'zh-TW', 'ja', 'ko',
  'fr', 'hi', 'it', 'pl', 'tr', 'vi', 'ar', 'uk', 'ru', 'nl',
  'id', 'th', 'ms', 'he', 'sv', 'pt', 'bn'
];
```

---

## Locale File Structure

Each locale file is a flat JSON object with **two-level nesting**: top-level namespace keys
containing flat string key-value maps. Some namespaces have one additional level of nesting
(e.g., `onboarding.welcome.title`), giving a maximum depth of 3.

### Top-Level Namespaces (67 total in en.json)

```
common              -- universal UI labels (Save, Cancel, Loading...)
time                -- relative time strings (just now, 5m ago...)
weekdays            -- abbreviated day names (Sun, Mon...)
months              -- full month names (January, February...)
nav                 -- main navigation labels
sidebar             -- sidebar UI strings
chat                -- chat pane strings
chatInput           -- chat composer strings
voiceDownload       -- voice model download dialog
approval            -- tool approval modal
browserExtension    -- browser extension prompts
commandPalette      -- command palette (Cmd+K)
whatsNew            -- update notification modal
upgradeSuccess      -- plan upgrade success modal
errorBoundary       -- error boundary fallback
sessionExpiry       -- session expiration warning
oauth               -- OAuth sign-in labels
notifications       -- notification bell dropdown
slashPicker         -- slash command picker
dataTable           -- generic data table
dateFilter          -- date range filter
statusBadge         -- status badge tooltips
theme               -- theme toggle labels
saveButton          -- save button states
searchInput         -- search input
tagInput            -- tag input
autocomplete        -- autocomplete widget
collapsibleCard     -- collapsible card
drawer              -- drawer / sidebar overlay
alertDialog         -- confirmation dialog
onboarding          -- 5-step onboarding wizard
  onboarding.welcome
  onboarding.terms
  onboarding.provider
  onboarding.apiKey
  onboarding.capabilities
  onboarding.neboloop
  onboarding.language
  onboarding.complete
  onboarding.capabilityNames
settings            -- settings shell and nav items
  settings.navItems
settingsAccount     -- account settings page
  settingsAccount.deleteModal
settingsAbout       -- about settings page
settingsAdvisors    -- advisor configuration page
  settingsAdvisors.roleOptions
settingsPlugins     -- plugin management page
settingsAgents      -- agent list page
settingsApps        -- apps management page
settingsBilling     -- billing and payment page
settingsDeveloper   -- developer tools page
settingsHeartbeat   -- heartbeat configuration
  settingsHeartbeat.intervals
settingsIdentity    -- agent identity page
settingsMemories    -- memory browser page
settingsNotes       -- environment notes page
settingsPermissions -- permissions configuration
settingsPersonality -- soul / personality page
  settingsPersonality.voiceOptions
  settingsPersonality.lengthOptions
  settingsPersonality.emojiOptions
  settingsPersonality.formalityOptions
  settingsPersonality.proactivityOptions
settingsProfile     -- user profile page
  settingsProfile.timezones
settingsProviders   -- AI provider configuration
  settingsProviders.providerOptions
settingsRouting     -- model routing configuration
  settingsRouting.modes
  settingsRouting.lanes
settingsRules       -- agent rules page
settingsSecrets     -- API key secrets page
settingsSessions    -- session history page
  settingsSessions.sources
settingsSkills      -- skills management page
settingsStatus      -- system status page
  settingsStatus.laneNames
settingsUsage       -- plan usage page
settingsFamily      -- family / parental controls
marketplace         -- marketplace pages
  marketplace.installedPage
  marketplace.agentsPage
  marketplace.skillsPage
  marketplace.detail
commander           -- commander org chart
agent               -- agent detail pages
agentSettings       -- agent settings form
agentConfigure      -- agent configuration
agentPersona        -- agent persona editor
agentActivity       -- agent activity / runs
automations         -- workflow automations
newBot              -- new agent creation menu
layout              -- layout-level messages
settingsAssistant   -- assistant settings links
```

### Key Count

The English reference file (`en.json`) contains **1,217 leaf translation keys** across
1,409 lines. Non-English locales have between 1,182 and 1,208 leaf keys, with 13-39
keys missing relative to `en.json` (see Key Coverage Analysis below).

---

## Translation Key Naming Patterns

### Convention

Keys follow a **namespace.camelCase** pattern:

```
{namespace}.{key}
{namespace}.{subNamespace}.{key}
```

- **Namespaces** are camelCase, typically matching the page or component name
- **Leaf keys** are camelCase, describing the UI element or purpose
- Maximum depth is 3 levels (e.g., `onboarding.welcome.title`)

### Examples

```
common.save                        -- universal "Save" button
chat.placeholder                   -- chat input placeholder
settings.navItems.account          -- settings nav item label
onboarding.provider.recommended    -- "Recommended" badge in onboarding
settingsProviders.providerOptions.anthropic  -- provider option label
marketplace.detail.ratings         -- "{count} Ratings" with interpolation
```

### Naming Patterns by Category

| Pattern | Example | Used For |
|---------|---------|----------|
| `{page}.title` | `settingsAccount.title` | Page heading |
| `{page}.description` | `settingsAccount.description` | Page subtitle |
| `{page}.loading` / `{page}.loadingX` | `settingsAgents.loading` | Loading state |
| `{page}.noX` / `{page}.noXHint` | `settingsAgents.noAgents` | Empty state |
| `{component}.placeholder` | `chatInput.placeholder` | Input placeholder |
| `{page}.{action}Failed` | `settingsProviders.deleteFailed` | Error messages |
| `{page}.{noun}Label` | `settingsIdentity.nameLabel` | Form field labels |
| `{page}.{noun}Placeholder` | `settingsIdentity.namePlaceholder` | Form placeholders |
| `{page}.{noun}Desc` | `settingsPermissions.autoShellDesc` | Descriptive text |
| `{feature}.{sub}.{key}` | `onboarding.terms.dataLocal` | Nested features |

---

## Translation Loading Strategy

### Lazy Loading via Dynamic Imports

All locales are registered with lazy-loading functions:

```typescript
register('en', () => import('./locales/en.json'));
register('de', () => import('./locales/de.json'));
// ... 23 more
```

This means:
1. **No locale data is bundled** into the main JavaScript bundle
2. Each locale JSON is a **separate chunk** created by Vite at build time
3. Only the active locale is loaded on first render
4. The fallback locale (`en`) is loaded when needed (if a key is missing)

### Load Sequence

```
1. Root layout imports '$lib/i18n'  (side-effect import)
2. index.ts executes:
   a. register() x25 -- registers lazy loaders, loads nothing yet
   b. detectLocale() -- determines which locale to use
   c. init() -- sets initialLocale and fallbackLocale
3. svelte-i18n internally calls the registered loader for initialLocale
4. Dynamic import() fetches the JSON chunk from the Vite dev server / built assets
5. Translations become available; $t() calls resolve
6. If a key is missing, svelte-i18n loads the fallback locale ('en') on demand
```

### Build-Time vs Runtime

| Aspect | Behavior |
|--------|----------|
| **Build time** | Vite code-splits each `*.json` into a separate chunk. No translations in main bundle. |
| **Runtime (dev)** | Vite serves JSON files on demand via HMR. Near-instant loading. |
| **Runtime (prod)** | Browser fetches the hashed chunk for the active locale. Cached by HTTP cache. |
| **SSR** | Locale is always `'en'` on server (`if (!browser) return 'en'`). Client hydration loads the user's actual locale. |

---

## Locale Detection and Selection

The `detectLocale()` function in `index.ts` implements a three-tier fallback:

```typescript
function detectLocale(): string {
    // 1. SSR guard -- always English on server
    if (!browser) return 'en';

    // 2. Explicit user preference (set during onboarding or settings)
    const saved = localStorage.getItem('nebo_locale');
    if (saved) return saved;

    // 3. Browser language detection
    const browserLang = navigator.language;  // e.g. 'fr-CA', 'en-US', 'ja'

    // 3a. Exact match
    if (supportedLocales.includes(browserLang)) return browserLang;

    // 3b. Base language match (strip region)
    const base = browserLang.split('-')[0];  // 'fr' from 'fr-CA'
    return supportedLocales.find(l => l === base || l.startsWith(base + '-')) ?? 'en';
}
```

### Priority Order

```
1. localStorage['nebo_locale']     -- Explicit user choice (survives browser restart)
2. navigator.language (exact)      -- Browser reports e.g. 'pt-BR', matches directly
3. navigator.language (base match) -- 'fr-CA' -> tries 'fr', finds it
4. 'en'                            -- Ultimate fallback
```

### Edge Cases

| Browser Language | Resolution | Result |
|-----------------|------------|--------|
| `en-US` | Not in list. Base `en` matches `en`. | `en` |
| `pt-BR` | Exact match in `supportedLocales`. | `pt-BR` |
| `pt-PT` | Not exact. Base `pt` matches `pt`. | `pt` |
| `zh` | Not exact. Base `zh` matches `zh-CN` (first match starting with `zh-`). | `zh-CN` |
| `zh-TW` | Exact match. | `zh-TW` |
| `fr-CA` | Not exact. Base `fr` matches `fr`. | `fr` |
| `da` | Not exact. No match for `da`. | `en` |
| `nb` | Not exact. No match for `nb`. | `en` |

---

## Runtime Locale Switching

### Onboarding Flow

During onboarding step 1 ("Choose Your Language"), the user selects from a grid of 25
languages. On confirmation:

```typescript
// app/src/routes/onboarding/+page.svelte
async function saveLocale() {
    localStorage.setItem('nebo_locale', selectedLocale);
    try {
        await api.userUpdatePreferences({ language: selectedLocale });
    } catch {
        logger.warn('Failed to save language preference to backend');
    }
    step = 2;
}
```

This writes to both localStorage (for immediate client-side use) and the backend SQLite
database (for persistence across devices, if applicable).

### Settings Profile (app-v1)

In the V1 app, the profile settings page allows changing language at runtime:

```typescript
// app-v1/src/routes/(app)/settings/profile/+page.svelte
locale.set(language);
localStorage.setItem('nebo_locale', language);
```

Calling `locale.set()` triggers svelte-i18n to:
1. Load the new locale's JSON (if not already loaded)
2. Update the `$t` store
3. All components using `{$t('...')}` reactively re-render with new translations

### V2 Profile Page Status

The current V2 profile page (`app/src/routes/settings/profile/+page.svelte`) does **not**
yet include a language picker. Language can currently only be set during onboarding or by
manually editing `localStorage['nebo_locale']`. This is a known gap (see Known Gaps below).

### Backend Sync

The backend persists the language preference via:

```
PUT /api/v1/user/me/preferences
Body: { "language": "fr" }
```

Handler: `crates/server/src/handlers/user.rs` -> `update_preferences()`

This stores the value in the SQLite `user_preferences` table. The V1 app fetches this
on load and calls `locale.set()` to sync. The V2 app currently only reads from
`localStorage` at boot time.

---

## Using Translations in Components

### Import Pattern

```svelte
<script lang="ts">
    import { t } from 'svelte-i18n';
</script>
```

### Usage in Templates

**Simple key lookup:**
```svelte
<h2>{$t('settings.title')}</h2>
<button>{$t('common.save')}</button>
```

**With variable interpolation:**
```svelte
<p>{$t('sidebar.activeCount', { values: { active: 3, total: 5 } })}</p>
<!-- Output: "3 of 5 active" -->
```

**In attributes:**
```svelte
<button aria-label={$t('common.refresh')}>...</button>
<input placeholder={$t('chatInput.placeholder')} />
```

**In JavaScript (confirm dialogs, etc.):**
```svelte
if (!confirm($t('marketplace.installedPage.uninstallConfirm', {
    values: { name: item.name }
}))) return;
```

### Current Adoption

As of V2, `$t()` is primarily used in:
- All marketplace pages (`/marketplace/**`)
- Onboarding flow (language selection)

Many V2 pages still use hardcoded English strings. The V1 app (`app-v1/`) has much broader
`$t()` adoption across all settings pages, navigation, commander, and agent pages.

---

## Variable Interpolation

Translations use ICU MessageFormat placeholders via svelte-i18n's integration with
`intl-messageformat`.

### Simple Variables

Locale file:
```json
{
    "sidebar": {
        "activeCount": "{active} of {total} active",
        "stepProgress": "Step {step} of {total}"
    }
}
```

Usage:
```svelte
{$t('sidebar.activeCount', { values: { active: 3, total: 5 } })}
```

### Interpolation Patterns Used in Nebo

| Pattern | Example Key | Example Value | Variables |
|---------|------------|---------------|-----------|
| Count | `time.minutesAgo` | `{n}m ago` | `n` |
| Multi-var | `time.hoursMinutes` | `{hrs}h {mins}m` | `hrs`, `mins` |
| Name insertion | `sidebar.deleteAgentConfirm` | `Are you sure you want to delete "{name}"?` | `name` |
| Version | `whatsNew.versionInfo` | `You're now running v{version}` | `version` |
| Stats | `settingsMemories.stats` | `{total} total \u00b7 {tacit} tacit \u00b7 {daily} daily \u00b7 {entity} entity` | `total`, `tacit`, `daily`, `entity` |
| Range | `dataTable.showing` | `Showing {start} to {end} of {total} results` | `start`, `end`, `total` |
| Money | `settingsUsage.price` | `${amount}/{interval}` | `amount`, `interval` |
| Percentage | `settingsAccount.sessionUsed` | `Session: {percent}% used` | `percent` |
| Channel | `chat.messageChannel` | `Message #{channel}...` | `channel` |

---

## Pluralization

### Current State

Nebo does **not** currently use ICU plural rules (`{count, plural, one {...} other {...}}`)
in its locale files. Instead, pluralization is handled at the component level using
conditional logic:

```svelte
{total === 1
    ? $t('marketplace.itemCountSingular', { values: { total } })
    : $t('marketplace.itemCount', { values: { total } })}
```

With separate keys for singular/plural:
```json
{
    "marketplace": {
        "itemCount": "{total} items",
        "itemCountSingular": "{total} item"
    }
}
```

### Limitation

This approach works for English-like languages (singular vs plural) but does not handle
languages with complex plural rules (e.g., Arabic has 6 plural forms, Polish has 3,
Russian has 3). Since svelte-i18n supports ICU plural syntax, this could be improved:

```json
{
    "marketplace": {
        "itemCount": "{total, plural, one {# item} other {# items}}"
    }
}
```

However, this would require updating all 25 locale files with language-specific plural
rules -- a non-trivial effort that has not been prioritized.

---

## Date, Time, and Number Formatting

### Current Approach

Nebo does **not** use svelte-i18n's built-in `$date`, `$number`, or `$time` formatters.
Instead, it uses a combination of:

1. **Translation keys for relative time** -- the `time` namespace provides pre-formatted
   relative time strings:
   ```json
   {
       "time": {
           "justNow": "just now",
           "minutesAgo": "{n}m ago",
           "hoursAgo": "{n}h ago",
           "daysAgo": "{n}d ago",
           "seconds": "{n}s",
           "minutesSeconds": "{mins}m {secs}s",
           "minutes": "{mins}m",
           "hoursMinutes": "{hrs}h {mins}m",
           "never": "Never"
       }
   }
   ```
   These are translated per locale (e.g., Arabic: `"minutesAgo": "منذ {n} د"`).

2. **Translation keys for calendar names** -- the `weekdays` and `months` namespaces:
   ```json
   {
       "weekdays": { "sun": "Sun", "mon": "Mon", ... },
       "months": { "january": "January", "february": "February", ... }
   }
   ```

3. **Browser-native `toLocaleTimeString()`** for clock-format times:
   ```typescript
   // app/src/lib/chat/controller.svelte.ts
   export function formatTime(ts: string | number): string {
       // ...
       return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
   }
   ```
   This automatically uses the browser's locale for time formatting (12h vs 24h, etc.).

4. **Custom `formatNumber()` functions** in marketplace pages:
   ```typescript
   function formatNumber(n: number) { ... }
   ```
   These are page-local and do not leverage svelte-i18n's number formatting.

5. **Custom `formatDate()` function** in billing page:
   ```typescript
   function formatDate(dateStr: string): string { ... }
   ```

### Implications

- Relative time display ("5m ago") is fully translated
- Calendar names (month and day names) are fully translated
- Clock-format times respect the browser's locale setting (independent of Nebo's i18n locale)
- Number formatting (thousand separators, decimal marks) is not locale-aware in custom formatters
- Currency formatting is not locale-aware (uses hardcoded `$` prefix)

---

## RTL Language Support

### Supported RTL Locales

Two RTL (right-to-left) languages are included:
- **Arabic (`ar`)** -- Arabic script
- **Hebrew (`he`)** -- Hebrew script

### Current RTL Implementation

There is **no automatic `dir="rtl"` attribute** set on the `<html>` or `<body>` element when
an RTL locale is active. The application does not currently:

- Toggle `dir="rtl"` on the document root
- Apply RTL-aware CSS (e.g., `rtl:` Tailwind variants)
- Mirror layout for RTL users

The translation strings themselves are correctly translated into Arabic and Hebrew (with proper
script and direction within text elements), but the overall page layout remains LTR.

### What Would Be Needed

To properly support RTL:
1. Detect RTL locale in the root layout:
   ```typescript
   const rtlLocales = ['ar', 'he'];
   $effect(() => {
       document.documentElement.dir = rtlLocales.includes($locale) ? 'rtl' : 'ltr';
   });
   ```
2. Add Tailwind RTL plugin or use logical properties (`ms-*`, `me-*`, `ps-*`, `pe-*`)
3. Audit all absolute directional classes (`ml-*`, `mr-*`, `pl-*`, `pr-*`, `text-left`, `text-right`)
4. Test with actual RTL users

---

## Missing Translation Fallback

### Behavior

svelte-i18n's fallback chain:

```
1. Look up key in the current locale's loaded messages
2. If not found, look up key in the fallback locale ('en')
3. If not found in fallback either, return the key path itself (e.g., "chat.newKey")
```

### Configuration

```typescript
init({
    fallbackLocale: 'en',
    initialLocale: detectLocale()
});
```

The fallback locale `'en'` is loaded on demand -- only when a key lookup in the active
locale fails. Once loaded, it is cached in memory.

### Practical Effect

If a German user encounters a key that exists in `en.json` but not in `de.json`, they will
see the English string. This is the intended graceful degradation -- English is always the
source of truth and the most complete locale.

If a key is missing from both the active locale AND `en.json`, the raw key path is displayed
(e.g., `"chat.missingKey"`). This is visible as a bug in the UI.

---

## Key Coverage Analysis

Based on comparison of all locale files against `en.json` (1,217 leaf keys):

| Locale Group | Key Count | Missing | Coverage |
|-------------|-----------|---------|----------|
| `en` (reference) | 1,217 | 0 | 100% |
| `th`, `bn`, `ms`, `id`, `he`, `sv`, `ru`, `pt` | 1,208 | 13 | 99.0% |
| All other locales (de, es, fr, ar, ...) | 1,182 | 39 | 96.8% |

### Common Missing Keys (across all non-English locales)

These keys exist in `en.json` but are missing from non-English files:
- `agentPersona.addProperty`
- `agentPersona.properties`
- `agentPersona.propertiesEmpty`
- `agentPersona.propertyName`
- `agentPersona.propertyValue`
- `agentPersona.removeProperty`

### Additional Missing Keys (in the 1,182-key locales)

These locales are also missing:
- `settings.navItems.agents`, `settings.navItems.plugins`, `settings.navItems.skills`
- `settingsAgents.*` (entire namespace -- 8 keys)
- `settingsPlugins.*` (entire namespace -- 14 keys)
- `settingsProviders.viewUsage`
- `settingsAccount.viewUsage`
- `settingsSkills.deleteTitle`
- `settingsUsage.activePool`, `budgetBalance`, `creditsPool`, `freePool`, `giftPool`

### Extra Keys (in non-English locales)

Some locales have 4 keys not present in `en.json`:
- `settingsUsage.extraUsage`
- `settingsUsage.extraUsageDesc`
- `settingsUsage.manageCredits`
- `settingsUsage.usageCount`

These appear to be remnants of previously removed English keys.

---

## Backend Language Persistence

### API Endpoint

```
PUT /api/v1/user/me/preferences
Content-Type: application/json

{
    "language": "fr",
    "theme": "dark",
    "timezone": "America/New_York"
}
```

### Backend Handler

File: `crates/server/src/handlers/user.rs`

```rust
pub async fn update_preferences(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    state.store.update_user_preferences(
        body["theme"].as_str(),
        body["language"].as_str(),    // <-- language stored here
        body["timezone"].as_str(),
        body["emailNotifications"].as_i64().map(|v| v != 0),
        body["inappNotifications"].as_i64().map(|v| v != 0),
    ).map_err(to_error_response)?;
    // ...
}
```

The language preference is stored in the SQLite `user_preferences` table at
`~/.nebo/data/nebo.db`.

### Dual Persistence Model

```
Client-side:  localStorage['nebo_locale']     -- fast, survives page reload
Server-side:  user_preferences.language        -- persistent, survives reinstall
```

The V1 app syncs from backend to client on page load. The V2 app currently only
reads from `localStorage` at boot (gap noted below).

---

## Cross-System Interactions

### Interaction Map

```
+-------------------+        +-------------------+        +------------------+
|  Onboarding Flow  |------->| localStorage      |------->| i18n detectLocale|
|  (language step)  |        | 'nebo_locale'     |        | (on app boot)    |
+-------------------+        +-------------------+        +------------------+
        |                                                         |
        v                                                         v
+-------------------+                                   +------------------+
| Backend API       |                                   | svelte-i18n      |
| /user/preferences |                                   | init() / $t()    |
+-------------------+                                   +------------------+
                                                                  |
                                                                  v
                                                        +------------------+
                                                        | All Components   |
                                                        | using {$t('...')}|
                                                        +------------------+
```

### Stores Involved

| Store | Type | Role |
|-------|------|------|
| svelte-i18n `locale` | Writable Svelte store | Current locale string |
| svelte-i18n `t` | Readable Svelte store | Translation function |
| svelte-i18n `isLoading` | Readable Svelte store | True while loading locale |
| `localStorage['nebo_locale']` | Browser storage | Persisted user preference |
| `theme` store (`$lib/stores/theme.js`) | Svelte store | Theme is adjacent, not i18n |

### Components That Interact with i18n

| Component/Module | Interaction |
|-----------------|-------------|
| `app/src/routes/+layout.svelte` | Imports `$lib/i18n` (triggers init) |
| `app/src/lib/i18n/index.ts` | Module entry point -- register, detect, init |
| `app/src/routes/onboarding/+page.svelte` | Language picker, writes `nebo_locale` + backend |
| All marketplace pages | Consume `$t()` for translated strings |
| `app-v1` settings/profile page | Language dropdown, calls `locale.set()` |
| `app-v1` root layout | Syncs backend language pref to `locale.set()` |
| Backend `user.rs` handler | Persists language preference to SQLite |

---

## Adding a New Locale

### Step-by-Step Guide

**1. Create the locale JSON file**

Copy `en.json` as a starting point:
```bash
cp app/src/lib/i18n/locales/en.json app/src/lib/i18n/locales/XX.json
```

Replace `XX` with the BCP 47 locale code (e.g., `da` for Danish, `fi` for Finnish).

**2. Translate all keys**

Translate every value in `XX.json`. Keep the JSON structure and key names identical to
`en.json`. Only values change.

Important:
- Preserve all `{variable}` placeholders exactly as-is
- Do not translate key names
- Keep the same nesting structure

**3. Register the locale loader**

In `app/src/lib/i18n/index.ts`, add a `register()` call:
```typescript
register('XX', () => import('./locales/XX.json'));
```

**4. Add to the supportedLocales array**

In the same file, add `'XX'` to the array:
```typescript
const supportedLocales = ['en', 'de', ..., 'XX'];
```

**5. Add to the onboarding language picker**

In `app/src/routes/onboarding/+page.svelte`, add an entry to the `languages` array:
```typescript
const languages = [
    // ... existing entries
    { code: 'XX', label: 'Native Language Name' },
];
```

Use the language's native name (e.g., `Dansk` not `Danish`).

**6. Verify**

- Set `localStorage['nebo_locale']` to `'XX'` in browser DevTools
- Reload the app
- Verify translated strings appear in the UI
- Check for missing keys (they will appear as English fallback text)

### Checklist

```
[ ] JSON file created at app/src/lib/i18n/locales/XX.json
[ ] All 1,217 keys translated (match en.json structure exactly)
[ ] register() call added in index.ts
[ ] Code added to supportedLocales array in index.ts
[ ] Entry added to onboarding language picker
[ ] If RTL language: document.dir handling added
[ ] Manual testing in browser with locale active
```

---

## Known Gaps and Migration Status

### V2 i18n Wiring Status

The V2-PLAN.md explicitly tracks this:

> **What works:** i18n: 25 languages via svelte-i18n, lazy-loaded, browser locale detection
>
> **What doesn't work:** i18n strings not yet wired to components (English hardcoded,
> translation files present)

This means:
- The i18n infrastructure is fully operational
- Locale files are complete and translated
- Most V2 components still use hardcoded English strings instead of `$t()` calls
- The marketplace pages are the exception -- they are fully wired with `$t()`

### Specific Gaps

| Gap | Description | Impact |
|-----|-------------|--------|
| **Hardcoded strings in V2** | Most V2 pages use English strings directly | Non-English users see English in most pages |
| **No language picker in V2 settings** | Profile page lacks language dropdown | Users can only set language during onboarding |
| **No RTL layout support** | `dir="rtl"` is never set | Arabic/Hebrew users see mirrored text in LTR layout |
| **No ICU pluralization** | Manual singular/plural split at component level | Incorrect plural forms in languages with complex rules |
| **No svelte-i18n date/number formatters** | Custom formatters, not locale-aware | Numbers/dates may not match user's locale expectations |
| **Backend-to-client sync missing in V2** | V2 does not fetch saved language from backend on boot | Locale could desync if localStorage is cleared |
| **39 missing keys in most locales** | Recently added keys not translated | English fallback shown for new features |
| **4 orphan keys in non-English locales** | Keys removed from English but still in translations | Dead translation data (no user impact) |

### Migration Path

The V2-PLAN.md lists the priority order:
1. Category Filtering
2. Chat Sending
3. Settings Save
4. **i18n wiring -- replace hardcoded English with `$t()` calls across all components**
5. P2/P3 items

---

## Key Files Reference

| File | Path | Purpose |
|------|------|---------|
| i18n module | `app/src/lib/i18n/index.ts` | Library init, locale registration, detection |
| Root layout | `app/src/routes/+layout.svelte` | Imports i18n module (triggers initialization) |
| English reference | `app/src/lib/i18n/locales/en.json` | Source of truth (1,217 keys, 1,409 lines) |
| Locale files (x25) | `app/src/lib/i18n/locales/*.json` | Per-language translations |
| Onboarding | `app/src/routes/onboarding/+page.svelte` | Language picker + localStorage + backend write |
| V1 Profile | `app-v1/src/routes/(app)/settings/profile/+page.svelte` | Runtime language switching (V1) |
| V2 Profile | `app/src/routes/settings/profile/+page.svelte` | No language picker yet (V2 gap) |
| Backend handler | `crates/server/src/handlers/user.rs` | `update_preferences()` stores language |
| Chat controller | `app/src/lib/chat/controller.svelte.ts` | `formatTime()` uses `toLocaleTimeString()` |
| V2 Plan | `app/V2-PLAN.md` | Documents i18n migration status and priorities |
