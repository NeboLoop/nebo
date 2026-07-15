# Nebo V2 — Style Guide

> **Audience:** Developers and AI coding assistants working on the Nebo SvelteKit app.
> **Stack:** SvelteKit · Tailwind CSS v4 · DaisyUI · Geist typeface
> **Theme:** Custom `clean` theme is the shipped light mode; `dark` is the shipped dark mode (both defined in `app.css`). All values are DaisyUI semantic tokens, so the guidance holds for every theme. (A `nebo` teal theme also exists in `app.css` but is not applied at runtime.)

---

## 1. Design Principles

Nebo is a **dense, professional productivity app** for non-technical users. The design should feel like a well-made desktop tool — structured, readable, and calm. Not flashy. Not corporate. Not toy-like.

Three qualities drive every decision:

- **Hierarchy** — the eye should immediately know what's primary, secondary, and metadata.
- **Restraint** — one accent color per screen. Depth from surface layering, not decorative shadows. No gratuitous animation.
- **Legibility** — text must always meet contrast minimums. Size and weight must vary meaningfully.

---

## 2. Color System

### Palette — DaisyUI Semantic Tokens Only

| Token | Light value (`clean`) | Role |
|---|---|---|
| `base-100` | `#ffffff` | Main panels, chat canvas, page background |
| `base-200` | `#fafafa` | Sidebar, recessed areas, canvas surfaces, selected state bg |
| `base-300` | `#ebebeb` | Borders, dividers, input outlines |
| `base-content` | `#1d1d1f` | All body text |
| `surface` | `#ffffff` | Elevated cards/inputs that must pop off `base-100` (e.g. chat composer) |
| `primary` | `#2563eb` | Cool blue — primary actions, links, active states |
| `primary-content` | `#ffffff` | Text on primary backgrounds |
| `accent` | `#e8503a` | Warm coral — badges, highlights, secondary CTAs |
| `accent-content` | `#ffffff` | Text on accent backgrounds |
| `secondary` | `#64748b` | Cool slate — rarely used directly |
| `neutral` | `#1d1d1f` | Darkest surface (tooltips, overlays) |
| `success` | `#22c55e` | Confirmations, online status |
| `warning` | `#f59e0b` | Running/in-progress states |
| `error` | `#ef4444` | Errors, destructive actions |
| `info` | `#3b82f6` | Informational callouts |

### Rules

- **Never hardcode hex values** in class attributes. All colors come from tokens above.
- **Opacity modifiers** create tints from tokens: `bg-primary/10`, `text-base-content/70`, `border-base-300/50`.
- **Primary vs Accent** — these are intentionally opposite temperatures (cool blue vs warm coral). Use `primary` for navigation, active states, links. Use `accent` for badges, tags, "new" labels, and CTAs that need to pop against the blue.
- **All themes inherit** — the token system means the app works correctly in dark, nord, sunset, coffee, etc. without any per-theme overrides.

### Surface Hierarchy

Surfaces are layered from deepest to highest:

```
border-base-300       ← borders and dividers
bg-base-200           ← sidebar, recessed panels, canvas/workspace areas, hover states
bg-base-100           ← main content panels, chat canvas
bg-surface            ← elevated cards/inputs that need to be distinctly white on base-100
bg-base-100 + shadow  ← elevated cards, modals, dropdowns (from --depth: 1)
bg-primary            ← active/selected avatars, primary buttons
```

`bg-surface` is defined as `#ffffff` in light themes and falls back to `base-100` in dark themes. Use it when an element sits on `bg-base-100` and must visually pop — the chat composer is the primary example. In dark themes the distinction is unnecessary because `base-100` is already the lightest surface, so `surface` maps to `base-100`.

If something feels flat, it's using the wrong surface tier. Check the hierarchy before adding ad hoc shadows.

### Agent Colors

Agents have a fixed palette of named colors. Each has a light-mode `bg` and `ink` CSS variable pair, and a dark-mode override. Use the token variables directly — never hardcode the hex values:

```svelte
<!-- Use the AGENT_COLORS_MAP or AGENT_COLORS utility from tokens.js -->
<div class="{agentColor.bgClass} {agentColor.inkClass}">N</div>
```

Available colors: `violet`, `green`, `sky`, `amber`, `rose`, `mint`, `slate`, `peach`, `lilac`.

---

## 3. Typography

Nebo uses a single typeface — **Geist** — for all text. **Geist Mono** for data and code. Visual hierarchy is created entirely through size, weight, and opacity. No color differences for type except meta/disabled states.

### Type Scale

| Role | Tailwind classes | Used for |
|---|---|---|
| **Page title** | `text-base font-semibold` | Panel/modal header titles |
| **Item title** | `text-sm font-medium` | Agent names, thread titles, card headings, nav labels |
| **Body** | `text-sm` | Descriptions, message content, form values, settings prose |
| **Secondary** | `text-xs text-base-content/70` | Role subtitles, preview snippets, supporting labels |
| **Meta** | `text-xs text-base-content/50 font-mono` | Timestamps, message counts, durations, run IDs |
| **Section label** | `text-xs font-semibold uppercase tracking-wider text-base-content/50` | Sidebar group headers, settings section dividers |
| **Code / mono** | `text-xs font-mono` | Status values, tool names, IDs, inline code |

### Critical Rule: Never use `text-sm` for secondary or meta content

`text-sm` is the body size. When secondary text (role labels, previews, timestamps) is also `text-sm`, all content reads at the same visual weight — the eye has no hierarchy to follow. This is the single biggest cause of "unpolished" UI.

**Correct pattern — agent list row:**
```svelte
<div class="text-sm font-medium">{agent.name}</div>
<div class="text-xs text-base-content/70">{agent.role}</div>
```

**Correct pattern — thread list row:**
```svelte
<div class="text-sm font-medium truncate">{thread.name}</div>
<div class="text-xs text-base-content/70 truncate">{thread.preview}</div>
<div class="text-xs text-base-content/50 font-mono">{thread.messages} messages · {thread.updatedAt}</div>
```

**Correct pattern — chat metadata:**
```svelte
<span class="text-xs text-base-content/50">Nebo worked for {duration}</span>
<span class="text-xs text-base-content/50 font-mono">Used {n} tools ↓</span>
<span class="text-xs text-base-content/50 font-mono">{msg.time}</span>
```

**Wrong (flat):**
```svelte
<div class="text-sm font-medium">{agent.name}</div>
<div class="text-sm text-base-content/70">{agent.role}</div>      <!-- ← should be text-xs -->
<div class="font-mono text-sm">{thread.updatedAt}</div>            <!-- ← should be text-xs -->
```

### Opacity Encodes Hierarchy

- `text-base-content` (full) — primary labels, item titles, active values
- `text-base-content/70` — secondary/supporting text
- `text-base-content/50` — meta, timestamps, placeholder-level content
- Never go below `/50` for readable text (contrast floor against `base-100` is approximately 4.5:1 at `/50`)

### Mono Is for Data, Not Prose

Use `font-mono` only for: timestamps, counts, IDs, durations, run labels, tool names, status codes. All prose and UI labels use the default body font.

---

## 4. Spacing, Layout & Two-Tone Rule

### The Three-Tone Pattern

The Schedule page is the reference implementation. Every page should mirror its structure:

| Zone | Token | Schedule equivalent | Agents page equivalent |
|---|---|---|---|
| **Primary nav sidebar** | `bg-base-200` + `border-r border-base-300` | Agent filter sidebar | Agent roster (col 1) |
| **Secondary chrome** | `bg-base-200/50` + `border-r border-base-content/10` | Time ruler | Thread list + header + tabs (col 2) |
| **Content** | `bg-base-100` | Calendar event well | Chat message canvas (col 3) |

The primary nav sidebar is the heavier `base-200`. The secondary chrome column (thread list, time ruler) uses the lighter `base-200/50` wash. Content is always `base-100`. Headers within content panels have no background — just `border-b border-base-content/10`.

**Canvas/workspace pages** (Team org chart, Automation builder):

| Zone | Token | Border |
|---|---|---|
| Primary nav sidebar | `bg-base-200` | `border-r border-base-300` |
| Canvas surface | `bg-base-100` | — |
| Cards / nodes on canvas | `bg-base-200/50` + depth shadow | `border border-base-300` |

### Hover States by Surface

This is the most commonly wrong thing. Hover must be visible — using `hover:bg-base-200` on a `base-200` background does nothing.

| Background | Hover class | Effect |
|---|---|---|
| `bg-base-200` (primary nav) | `hover:bg-base-100/70` | Lifts row slightly — card-lift feel |
| `bg-base-200/50` (secondary chrome) | `hover:bg-base-200` | Adds a bit of weight |
| `bg-base-100` (content panel) | `hover:bg-base-200/50` | Gentle tint |

### Selected State by Surface

| Background | Selected classes |
|---|---|
| `bg-base-200` (primary nav) | `bg-base-100 border border-base-300 shadow-sm` — card lifts off |
| `bg-base-200/50` (secondary chrome / thread list) | `border-l-2 border-l-primary` — accent line only, no bg change |
| `bg-base-100` (content panel) | `bg-base-200/50` — subtle fill |

Always pair a selected border with `border border-transparent` (or `border-l-2 border-l-transparent`) on inactive items to prevent layout shift.

### Density

Nebo is a **dense app**. Padding should be tight but not cramped. Reference values:

| Context | Padding |
|---|---|
| Panel header (h-11) | `px-3.5` |
| Sidebar list row | `py-2 px-2.5` |
| Thread/item row | `py-2.5 px-3.5` |
| Card interior | `p-3.5` or `p-4` |
| Modal interior | `px-5 py-4` |
| Section header label | `px-3.5 pt-3 pb-1` |
| Form field | `py-[7px] px-2.5` |

### Three-Column Layout (main Agents page)

```
[Col 1: 260px | bg-base-200]  Agent roster + status
[Col 2: 260px | bg-base-200/50]  Threads / Runs / Settings list
[Col 3: flex-1 | bg-base-100] Chat canvas (messages max-w-3xl centered)
                               Creations panel opens on right (50/50 split)
```

Col 1 collapses to a 48px icon rail. All columns use `border-r border-base-300` to separate. Chat messages and the composer are constrained to `max-w-3xl mx-auto` so the conversation stays readable at wide viewports. When the Creations panel is open, the chat column shrinks to `w-1/2` and the creations panel takes the remaining space.

### Two-Column Layout (Schedule, Team, Workspaces)

```
[Left: 210px | bg-base-200]  Nav / filter list
[Right: flex-1 | bg-base-100] Canvas/workspace
                               Cards and nodes use bg-base-200/50
```

Org chart nodes and workspace cards are `bg-base-200/50` — they appear as a subtle tint against the `bg-base-100` canvas behind them.

### Gap Rhythm

- Between list items: `gap-px` (hairline) or `gap-0.5`
- Between cards in a grid: `gap-3` or `gap-4`
- Between form fields: `gap-4` or `gap-5`
- Between section groups: `pt-3` spacer or `h-3` div

---

## 5. Component Patterns

### Sidebar List Row — Primary Nav (`bg-base-200`)

Hover lifts the row. Selected state is a card lifted off the surface.

```svelte
<button class="w-full flex items-center gap-2.5 py-2 px-2.5 mx-1.5 rounded-box cursor-pointer transition-colors text-left
  {active
    ? 'bg-base-100 border border-base-300 shadow-sm'
    : 'border border-transparent hover:bg-base-100/70'}">
  <div class="w-8 h-8 rounded-field flex items-center justify-center font-mono text-sm font-semibold shrink-0
    {active ? 'bg-primary text-primary-content' : 'border border-base-300 bg-base-100'}">
    {initial}
  </div>
  <div class="flex-1 min-w-0">
    <div class="text-sm font-medium truncate">{name}</div>
    <div class="text-xs text-base-content/70 truncate">{subtitle}</div>
  </div>
  <div class="w-[7px] h-[7px] rounded-full shrink-0 {statusColor}" title={statusLabel}></div>
</button>
```

### Thread / Secondary Chrome Row (`bg-base-200/50`)

Hover adds weight. Selected state is a thin primary left border — no bg change.

```svelte
<button class="w-full text-left py-2.5 px-3.5 border-b border-base-content/10 cursor-pointer transition-colors
  {active
    ? 'border-l-2 border-l-primary'
    : 'border-l-2 border-l-transparent hover:bg-base-200'}">
  <div class="text-sm font-medium truncate mb-0.5">{thread.name}</div>
  <div class="text-xs text-base-content/70 truncate mb-0.5">{thread.preview}</div>
  <div class="text-xs text-base-content/50 font-mono">{thread.messages} messages · {thread.updatedAt}</div>
</button>
```

### Section Header (sidebar group label)

```svelte
<div class="px-3.5 pt-3 pb-1 text-xs font-semibold uppercase tracking-wider text-base-content/50">
  {label}
</div>
```

### Card (elevated content — on any surface)

```svelte
<!-- On bg-base-100 canvas (org chart node, workspace card) -->
<div class="rounded-box border border-base-300 bg-base-200/50 p-4 shadow-sm">
  <!-- Subtle tint floats off the base-100 canvas -->
</div>

<!-- On bg-base-100 panel (settings card, info card) -->
<div class="rounded-box border border-base-300 bg-base-100 p-4">
  <!-- depth: 1 adds shadow automatically -->
</div>

<!-- Elevated input card on bg-base-100 (chat composer) -->
<div class="rounded-box border border-base-300 bg-surface shadow-sm p-3">
  <!-- Pure white in light themes, matches base-100 in dark themes -->
</div>
```

### Badge / Tag

```svelte
<!-- Neutral -->
<span class="py-0.5 px-2 rounded bg-base-200 font-mono text-xs text-base-content/70">{label}</span>

<!-- Primary -->
<span class="py-0.5 px-2 rounded bg-primary/10 text-primary font-mono text-xs">{label}</span>

<!-- Accent (use for "new", "marketplace", highlights) -->
<span class="py-0.5 px-2 rounded bg-accent/10 text-accent font-mono text-xs">{label}</span>
```

### Status Dot

Always pair with a label or title attribute — color alone is not enough:

```svelte
<div class="w-[7px] h-[7px] rounded-full {color}" title={statusLabel}></div>

<!-- Colors -->
bg-success                   ← online
bg-warning animate-pulse     ← running
bg-base-content/30           ← idle/offline
```

### Form Field

```svelte
<label class="block">
  <span class="block text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">
    {label}
  </span>
  <input type="text"
    class="w-full py-[7px] px-2.5 rounded-field border border-base-300 text-sm bg-base-100 outline-none
           focus:border-primary/50 transition-colors font-body" />
</label>
```

### Modal

```svelte
<div class="fixed inset-0 z-50 flex items-center justify-center">
  <div class="absolute inset-0 bg-black/30" role="presentation"></div>  <!-- No onclick — modals require explicit dismiss via X button -->
  <div class="relative bg-base-100 rounded-box border border-base-300 shadow-xl w-[620px] max-h-[80vh] flex flex-col z-10">
    <!-- Header -->
    <div class="flex items-center justify-between px-5 py-3.5 border-b border-base-300 shrink-0">
      <span class="text-base font-semibold">{title}</span>
      <button class="w-7 h-7 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none text-lg" onclick={close}>×</button>
    </div>
    <!-- Body -->
    <div class="flex-1 overflow-y-auto p-5">...</div>
    <!-- Footer -->
    <div class="flex items-center justify-end gap-2 px-5 py-3 border-t border-base-300 shrink-0">
      <button class="btn btn-ghost btn-sm" onclick={close}>Cancel</button>
      <button class="btn btn-primary btn-sm">Save</button>
    </div>
  </div>
</div>
```

### Primary Button

```svelte
<button class="btn btn-primary btn-sm">Label</button>

<!-- Ghost/secondary -->
<button class="btn btn-ghost btn-sm">Label</button>

<!-- Destructive -->
<button class="btn btn-error btn-sm btn-outline">Delete</button>
```

### Chat Message — User Bubble

```svelte
<div class="max-w-[640px] self-end mt-3">
  <div class="py-2.5 px-3.5 rounded-xl rounded-br-sm text-sm leading-relaxed bg-base-200">
    {content}
  </div>
  <div class="flex items-center gap-2 justify-end mt-1">
    <!-- action icons -->
    <span class="text-xs text-base-content/50 font-mono">{time}</span>
  </div>
</div>
```

### Chat Message — Assistant

```svelte
<div class="max-w-[640px] mt-3">
  <div class="text-sm leading-relaxed">{content}</div>
  <div class="flex items-center gap-2 mt-1.5">
    <!-- copy icon -->
    <span class="text-xs text-base-content/50 font-mono">{time}</span>
  </div>
</div>
```

### Chat Composer (elevated input card)

The composer card uses `bg-surface` — pure white that pops off the `bg-base-100` canvas. In dark themes, `surface` falls back to `base-100`.

```svelte
<div class="max-w-3xl mx-auto w-full">
  <div class="px-6 py-3 shrink-0">
    <div class="rounded-box border border-base-300 bg-surface shadow-sm p-3 relative">
      <textarea class="w-full text-base outline-none resize-none bg-transparent leading-snug"></textarea>
      <!-- attach button, send button -->
    </div>
  </div>
</div>
```

### Creations Panel (right side of chat)

When an agent generates a document, sheet, image, or report, the creations panel opens alongside the chat. The chat column shrinks to `w-1/2` and the creations panel takes `flex-1`.

```svelte
<!-- Creations panel -->
<div class="flex-1 min-w-[360px] flex flex-col border-l border-base-300 bg-base-100 min-h-0">
  <div class="h-11 px-4 border-b border-base-content/10 flex items-center gap-2 shrink-0">
    <span class="text-sm font-semibold flex-1 truncate">{title}</span>
    <button class="..." onclick={close}>×</button>
  </div>
  <div class="flex-1 overflow-y-auto p-6"><!-- content --></div>
</div>
```

### Chat Metadata (thinking / tools)

```svelte
<!-- Thinking summary -->
<details class="max-w-[640px] mt-2 mb-1">
  <summary class="text-xs text-base-content/50 cursor-pointer hover:text-base-content/70 transition-colors">
    Nebo worked for {duration}
  </summary>
  <div class="mt-1.5 py-2 px-3 rounded-box bg-base-200 border-l-2 border-base-content/20 text-xs leading-relaxed font-mono whitespace-pre-wrap">
    {thinkingContent}
  </div>
</details>

<!-- Tool use -->
<button class="flex items-center gap-1.5 text-xs text-base-content/50 cursor-pointer hover:text-base-content/70 transition-colors bg-transparent border-none p-0">
  Used {n} tools ↓
</button>
```

---

## 6. Borders & Dividers

- Use `border-base-300` for all structural borders (panel edges, card outlines, input outlines).
- Use `border-base-content/10` for very subtle separators (within a surface, not between surfaces).
- Use `divide-base-300` for list dividers where appropriate.
- **Border consistency rule:** if an element has a border in its active state, give it `border border-transparent` in its inactive state to prevent layout shift.

---

## 7. Accessibility

- **Contrast:** Normal text ≥ 4.5:1 against its background. Large text and interactive boundaries ≥ 3:1.
- **Color alone is never a signal.** Every status dot, badge color, or state change must be accompanied by a label, icon, or shape difference.
- **Interactive elements** must have visible focus states (DaisyUI provides these — do not remove `outline-none` without replacing it).
- **Status dots** must have a `title` attribute with the human-readable status label.
- **Icon-only buttons** must have `title` or `aria-label`.

---

## 8. Depth & Elevation

`--depth: 1` is enabled in the `nebo` theme. This means DaisyUI components (cards, dropdowns, modals) automatically get a layered shadow. Do not fight this with `shadow-none`.

Elevation layers (lowest to highest):
1. `bg-base-200` — recessed / sidebar / canvas
2. `bg-base-100` — main surface / list panels
3. `bg-base-100` + DaisyUI depth shadow — cards, popovers, nodes on a canvas
4. `bg-base-100` + `shadow-xl` — modals, dialogs (full overlay)

---

## 9. Animation

Keep animation purposeful and brief:

- Transitions: `transition-colors` for color changes, `transition-all duration-150` for layout shifts.
- Pulse: `animate-pulse` only for "running" status dots.
- Cursor blink: `animate-blink` (defined in `app.css`) for text cursors.
- No decorative animation — no floating elements, no entrance animations on page load.

---

## 10. File & Code Conventions

- All styles go in `app.css`. Never use `<style>` blocks in `.svelte` files.
- Component files live in `src/lib/components/`.
- Route files live in `src/routes/`.
- Mock data lives in `src/lib/mockData.js` and `src/lib/data.js`.
- Token helpers (agent colors, etc.) live in `src/lib/tokens.js`.
- Stores live in `src/lib/stores/`.
