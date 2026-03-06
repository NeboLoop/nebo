# Browser Extension Parity: Nebo vs Claude

Deep technical comparison of Nebo's browser extension (`chrome-extension/`) against
Claude's extension (`fcoeoabgfenejglbffodgkkbkcdhcgfn/1.0.57_0/`). Every gap is a
reliability or capability issue that must be closed.

---

## 1. CRITICAL: Element Reference Stability

### Claude's approach
Claude **never resets** the element map between reads. `window.__claudeElementMap` is a
plain object that persists across calls. When generating the tree, Claude checks if an
element already has a ref before assigning a new one:

```javascript
// Claude: reuse existing ref if element already mapped
var u = null;
for (var d in window.__claudeElementMap)
  if (window.__claudeElementMap[d].deref() === e) { u = d; break; }
u || (u = "ref_" + ++window.__claudeRefCounter,
       window.__claudeElementMap[u] = new WeakRef(e));
```

After generation, it cleans up only dead refs:
```javascript
for (var d in window.__claudeElementMap)
  window.__claudeElementMap[d].deref() || delete window.__claudeElementMap[d];
```

### Nebo's approach (BROKEN)
Nebo **resets everything** on every call:
```typescript
// Nebo: DESTROYS all refs on every read
refCounter = 0
elementMap.clear()
```

### Impact
- Agent reads page, gets `ref_5` = search button.
- Agent decides to click `ref_5`.
- Between read and click, if ANYTHING triggers a re-read, `ref_5` now points to a
  completely different element.
- Even without re-reads, the refs from the accessibility tree output don't survive the
  next `read_page` call. If the agent reads, thinks, then reads again to verify, all
  refs from the first read are gone.

### Fix required
- Use a plain object (not Map) for `__neboElementMap` to match Chrome serialization
- Do NOT clear the map on each generation
- Search for existing refs before creating new ones
- Only clean up dead WeakRefs after generation

---

## 2. CRITICAL: Select Option Enumeration

### Claude's approach
Claude explicitly enumerates all `<option>` elements inside a `<select>`:

```javascript
if ("select" === e.tagName.toLowerCase())
  for (var f = e.options, m = 0; m < f.length; m++) {
    var s = f[m],
        p = " ".repeat(t + 1) + "option",
        v = s.textContent ? s.textContent.trim() : "";
    v && (p += ' "' + v.replace(/\s+/g, " ").substring(0, 100).replace(/"/g, '\\"') + '"');
    s.selected && (p += " (selected)");
    s.value && s.value !== v && (p += ' value="' + s.value.replace(/"/g, '\\"') + '"');
    n.push(p);
  }
```

Output example:
```
combobox "California" [ref_12]
  option "Alabama" value="AL"
  option "California" (selected) value="CA"
  option "New York" value="NY"
```

### Nebo's approach (MISSING)
Nebo does NOT enumerate options at all. A `<select>` just shows as:
```
combobox "California" [ref_12]
```

### Impact
The agent has no way to know what options are available in a dropdown. It can't fill
forms with select elements effectively. This is a **fundamental gap** for form automation.

### Fix required
After generating the line for a `<select>` element, iterate its `.options` and push
child lines showing text, selected state, and value.

---

## 3. CRITICAL: Accessible Name Resolution Priority

### Claude's priority order
1. **Select option text** (if `<select>`, get selected option's textContent)
2. `aria-label`
3. `placeholder`
4. `title`
5. `alt`
6. `label[for="id"]`
7. Input `.value` (if < 50 chars)
8. Button/link/summary **direct child text nodes only** (NOT deep textContent)
9. Heading textContent (max 100 chars)
10. Generic direct child text nodes (min 3 chars, max 100)

Key detail: Claude reads **direct child text nodes** for buttons/links, not
`textContent` (which includes all descendant text). This avoids pulling in text from
nested icons, badges, etc.

```javascript
// Claude: only direct text nodes for button/a/summary
if (["button", "a", "summary"].includes(t)) {
  for (var h = "", g = 0; g < e.childNodes.length; g++) {
    var m = e.childNodes[g];
    m.nodeType === Node.TEXT_NODE && (h += m.textContent);
  }
  if (h.trim()) return h.trim();
}
```

### Nebo's priority order
1. `aria-label`
2. `aria-labelledby` (Claude doesn't have this!)
3. `label[for="id"]`
4. Parent `<label>` (Claude doesn't have this!)
5. `placeholder`
6. `title`
7. `alt`
8. Input value (submit/button/reset types only)
9. Deep `textContent` for specific tags (up to 200 chars)

### Gaps
- Nebo is MISSING: select option text as name, direct text node extraction
- Nebo HAS but Claude doesn't: `aria-labelledby`, parent label resolution
- Nebo uses deep `textContent` which can pull in garbage text from nested elements

### Fix required
- Add select option text check at position 1
- For button/a/summary, use direct child text nodes instead of `textContent`
- Keep `aria-labelledby` and parent label (Nebo is better here)

---

## 4. CRITICAL: Visibility and Viewport Filtering

### Claude's approach
Claude only applies visibility/viewport checks when `filter !== 'all'`:

```javascript
// Claude: in 'all' mode, include EVERYTHING (even hidden/offscreen)
if ("all" !== t.filter && "true" === e.getAttribute("aria-hidden")) return false;
if ("all" !== t.filter && !m(e)) return false;  // visibility check
if ("all" !== t.filter && !t.refId) {            // viewport check
  var i = e.getBoundingClientRect();
  if (!(i.top < window.innerHeight && i.bottom > 0 &&
        i.left < window.innerWidth && i.right > 0)) return false;
}
```

### Nebo's approach (DIFFERENT)
Nebo applies visibility checks always and viewport checks always (except focused subtree):

```typescript
// Nebo: always checks visibility
if (el.getAttribute('aria-hidden') === 'true') return  // always
if (!isVisible(el)) return                               // always
if (!focusRef && !isInViewport(el)) return               // always unless focused
```

### Impact
- In `all` mode, Nebo misses hidden elements that might be important (modals about to
  open, off-screen content the agent should know about, dropdowns)
- Claude lets `all` mode truly return everything, leaving filtering to the agent

### Fix required
- Only apply visibility/viewport filtering when `filter !== 'all'`
- Keep viewport filtering for `interactive` mode

---

## 5. IMPORTANT: Element Inclusion Logic

### Claude's approach
Claude uses a `shouldInclude` function with clear logic:
```javascript
function w(e, t) {
  // Skip script/style/meta etc
  if (["script","style","meta","link","title","noscript"].includes(r)) return false;

  // Visibility checks (only in non-all mode, see above)

  // Interactive filter: only interactive elements
  if ("interactive" === t.filter) return s(e);

  // For 'all' mode: interactive OR structural OR has name OR has non-generic role
  if (s(e)) return true;        // interactive
  if (p(e)) return true;        // structural (h1-h6, nav, main, etc)
  if (g(e).length > 0) return true;  // has accessible name
  var n = h(e);
  return null !== n && "generic" !== n && "image" !== n;  // has meaningful role
}
```

### Nebo's approach
Nebo has a more convoluted check:
```typescript
const skip = !role && !name && !['div', 'span', 'section', ...].includes(tag)
if (filter === 'interactive' && !isInteractive(el) && !skip) { /* walk children only */ }
if (role || (name && filter === 'all')) { /* include */ }
```

### Issues
- Nebo's `skip` variable logic is inverted/confusing
- Elements with names but no role are only included in `all` mode
- In `interactive` mode, non-interactive elements with names are skipped entirely
  (children not walked) when `skip` is true

### Fix required
Match Claude's clean inclusion logic: interactive OR structural OR has-name OR
has-meaningful-role.

---

## 6. IMPORTANT: Interactive Element Detection

### Claude's interactive check
```javascript
function s(e) {
  var t = e.tagName.toLowerCase();
  return ["a","button","input","select","textarea","details","summary"].includes(t)
    || null !== e.getAttribute("onclick")
    || null !== e.getAttribute("tabindex")
    || "button" === e.getAttribute("role")
    || "link" === e.getAttribute("role")
    || "true" === e.getAttribute("contenteditable");
}
```

### Nebo's interactive check
```typescript
function isInteractive(el: Element): boolean {
  if (INTERACTIVE_TAGS.has(tag)) return true
  if (el.getAttribute('tabindex') !== null) return true
  if (el.getAttribute('contenteditable') === 'true') return true
  if (el.getAttribute('onclick') !== null) return true
  if (el.getAttribute('role') && ['button','link','checkbox','radio','tab',
      'menuitem','option','switch','textbox','combobox','searchbox','slider']
      .includes(el.getAttribute('role')!)) return true
  if (getComputedStyle(el).cursor === 'pointer') return true
  return false
}
```

### Differences
- Nebo checks MORE interactive ARIA roles (checkbox, radio, tab, menuitem, etc) - GOOD
- Nebo checks `cursor: pointer` via getComputedStyle - GOOD but potentially SLOW
  (forces style recalc on every element)
- Claude is more conservative but faster

### Action
Keep Nebo's expanded role list. Consider removing `cursor: pointer` check or making it
opt-in, as it triggers expensive style recalcs.

---

## 7. IMPORTANT: Role Mapping

### Claude's role map
```javascript
{ a:"link", button:"button",
  input: (type-based: submit/button→"button", checkbox→"checkbox",
          radio→"radio", file→"button", default→"textbox"),
  select:"combobox", textarea:"textbox",
  h1-h6:"heading", img:"image",
  nav:"navigation", main:"main", header:"banner", footer:"contentinfo",
  section:"region", article:"article", aside:"complementary",
  form:"form", table:"table", ul:"list", ol:"list", li:"listitem",
  label:"label"
}
// Default: "generic" (not empty string)
```

### Nebo's role map (superset)
```typescript
{ // Everything Claude has PLUS:
  option:"option", dialog:"dialog", details:"group", summary:"button",
  p:"paragraph", tr:"row", th:"columnheader", td:"cell",
  fieldset:"group", legend:"legend", progress:"progressbar",
  meter:"meter", video:"video", audio:"audio", iframe:"iframe"
}
// Default: "" (empty string)
```

### Differences
- Nebo maps MORE tags - this is GOOD
- Claude defaults to `"generic"` for unmapped tags, Nebo to `""` (empty)
- Claude has `img:"image"`, Nebo has `img:"img"` - inconsistent

### Fix required
- Change default from `""` to `"generic"` (matches WAI-ARIA spec for `<div>`, `<span>`)
- Change `img:"img"` to `img:"image"` (matches ARIA role name)

---

## 8. IMPORTANT: Attribute Output

### Claude's attributes
For each tree line, outputs: `href`, `type`, `placeholder`

### Nebo's attributes
For each tree line, outputs: `href`, `checked`, `disabled`, `required`, `readonly`,
`aria-expanded`, `aria-selected`, `aria-pressed`

### Assessment
Nebo outputs MORE useful state attributes. This is BETTER than Claude. Keep this.

Add `type` and `placeholder` to match Claude (useful for the agent to know input types).

---

## 9. IMPORTANT: Character Limit Handling

### Claude's approach (BETTER)
Generates full tree, then checks length. If over limit, returns a helpful error:

```javascript
if (null != r && c.length > r) {
  var f = "Output exceeds " + r + " character limit (" + c.length + " characters). ";
  f += i ? "The specified element has too much content. Try specifying a smaller depth..."
       : void 0 !== t ? "Try specifying an even smaller depth parameter..."
       : "Try specifying a depth parameter (e.g., depth: 5)...";
  return { error: f, pageContent: "", viewport: {...} };
}
```

### Nebo's approach
Checks during generation, stops early:
```typescript
if (maxChars > 0 && charCount.current >= maxChars) return
```

### Impact
Nebo silently truncates output. The agent doesn't know the tree was cut short or how
much was missing. Claude tells the agent exactly what happened and what to do about it.

### Fix required
Generate full tree first, then check length. Return descriptive error with suggestions
if over limit.

---

## 10. IMPORTANT: RefId Focused Subtree

### Claude's approach (BETTER)
When `refId` is specified, Claude looks up the element directly from the persistent map:

```javascript
if (i) {
  var l = window.__claudeElementMap[i];
  if (!l) return {
    error: "Element with ref_id '" + i + "' not found. It may have been removed...",
    pageContent: "", viewport: {...}
  };
  var u = l.deref();
  if (!u) return {
    error: "Element with ref_id '" + i + "' no longer exists...",
    pageContent: "", viewport: {...}
  };
  b(u, 0, o);  // Start traversal directly from that element
}
```

### Nebo's approach (BROKEN)
Nebo clears the map first (destroying all refs), then tries to find the ref by walking
the entire tree:

```typescript
// 1. Clear everything (oops, refId is gone now)
refCounter = 0
elementMap.clear()

// 2. Walk tree trying to find focusRef
// This can NEVER work because the map was just cleared
```

### Impact
The `refId` parameter literally cannot work in Nebo. It was always broken.

### Fix required
Do NOT clear the map. Look up refId directly from the persistent map. Return descriptive
error if not found or GC'd. Start traversal from that element.

---

## 11. IMPORTANT: Tree Output Format

### Claude's format
```
button "Click me" [ref_123]
  link "Home" [ref_124] href="/"
  textbox [ref_125] type="text" placeholder="Search"
  combobox "California" [ref_126]
    option "Alabama" value="AL"
    option "California" (selected) value="CA"
```

No page title/URL header. Indentation increases for child elements of included parents.

### Nebo's format
```
page "Page Title" url="https://example.com"
  button "Click me" [ref_1]
    link "Home" [ref_2] href="/"
    textbox [ref_3] required
    combobox "California" [ref_4]
```

Includes page title/URL header. But missing option enumeration.

### Assessment
Nebo's page header is GOOD — keep it. Missing options is BAD — add it.

### Indentation difference
Claude only increases indent when the parent was included in the output. Nebo always
increments depth, so children of excluded parents appear at wrong depth.

```typescript
// Claude: only increment depth when parent was included
b(e.children[_], i ? t+1 : t, r)  // t+1 if parent included, t if skipped
```

```typescript
// Nebo: always increments
walkDOM(child, filter, depth + 1, ...)  // always depth+1
```

### Fix required
Only increment depth when the current element was included in output.

---

## 12. CRITICAL: Page Settle / Hydration Strategy

### Claude's approach (MUCH BETTER)
Claude uses **intelligent polling** instead of a fixed delay. After any action that
changes the page, it polls `document.readyState` AND `document.getAnimations()`:

```javascript
// Claude: poll until page is truly ready (not just loaded)
const expression =
  "document.readyState === 'complete' && document.getAnimations().length === 0";

// Poll every 50ms until both conditions are true, or timeout
for (; Date.now() - t < n; ) {
  const t = await Promise.race([
    g.sendCommand(a, "Runtime.evaluate", {
      expression, returnByValue: true
    }),
    new Promise(t => setTimeout(() => t(null), e))  // per-poll timeout
  ]);
  if (true === t?.result?.value) break;
  await new Promise(e => setTimeout(e, 50));  // 50ms between polls
}
```

**Timing varies by action type:**
```javascript
function ni(e) {
  const t = new Set(e.map(e => e.type));
  return t.has("left_click")
    ? { minMs: 200, maxMs: 500 }   // clicks: wait 200ms min, poll up to 500ms
    : t.has("js")
      ? { minMs: 100, maxMs: 500 }  // JS eval: wait 100ms min, poll up to 500ms
      : t.has("navigate") || t.has("new_tab")
        ? { minMs: 0, maxMs: 500 }  // navigate: no min wait, poll up to 500ms
        : t.has("scroll")
          ? { minMs: 100, maxMs: 0 } // scroll: wait 100ms, no polling
          : { minMs: 0, maxMs: 0 };  // other: no wait
}
```

**The key insight**: `document.getAnimations().length === 0` catches:
- CSS transitions (React/Svelte route transitions)
- CSS animations (loading spinners, skeleton screens)
- Web Animations API usage
- Framework hydration that triggers animations

This is far more reliable than a fixed delay because it adapts to the actual page state.

### Nebo's approach (NAIVE)
Fixed 500ms delay after `Page.loadEventFired`:
```typescript
// Nebo: fixed delay, hope for the best
await loaded  // Page.loadEventFired or 15s timeout
await new Promise(r => setTimeout(r, 500))  // blind 500ms
```

### Impact
- On fast sites (static HTML): wastes 500ms every navigation
- On slow SPA sites (React with lazy loading): 500ms is not enough, page still hydrating
- On animation-heavy sites: reads page mid-transition, gets garbage
- No adaptation to action type — scroll doesn't need 500ms, clicks might need more

### Fix required
Replace fixed delay with polling loop:
1. After navigation: poll `readyState === 'complete' && getAnimations().length === 0`
   every 50ms, with 500ms max timeout
2. After clicks: add 200ms minimum wait before polling (to let event handlers run)
3. After scroll: 100ms fixed delay (no polling needed)
4. After evaluate/fill: 100ms min wait + poll up to 500ms
5. Use `Promise.race` with per-poll timeout to avoid hanging on dead pages

---

## 13. CRITICAL: Text Escaping in Tree Output

### Claude's approach
Claude escapes quotes and normalizes whitespace in accessible names:
```javascript
// Claude: escape quotes and normalize whitespace
c += ' "' + (l = l.replace(/\s+/g, " ").substring(0, 100)).replace(/"/g, '\\"') + '"';
```

### Nebo's approach (BROKEN)
Nebo does NOT escape quotes or normalize whitespace:
```typescript
// Nebo: no escaping
const nameStr = name ? ` "${name}"` : ''
const line = `${indent}${role}${nameStr}${refStr}${attrStr}`
```

### Impact
If an element's name contains double quotes or newlines, the tree output becomes
malformed and unparseable by the agent. Example:
```
button "Click "here" now" [ref_5]     ← agent can't parse this
button "Multi
line text" [ref_6]                     ← broken tree structure
```

### Fix required
- Normalize whitespace: `name.replace(/\s+/g, ' ')`
- Escape quotes: `name.replace(/"/g, '\\"')`
- Truncate to 100 chars (currently 200 in Nebo, 100 in Claude)

---

## 14. CRITICAL: React/Vue/Angular Controlled Input Fill

### Current Nebo fill implementation
```typescript
el.focus()
el.value = val
el.dispatchEvent(new Event('input', { bubbles: true }))
el.dispatchEvent(new Event('change', { bubbles: true }))
```

### Problem
React overrides the `value` property descriptor on input elements. Setting `.value`
directly bypasses React's internal state, so React doesn't know the value changed.
The dispatched `input` event has no effect because React checks its own internal value
tracker.

### Fix required
Use the native property descriptor to bypass React's override:

```typescript
// Get the native setter from HTMLInputElement.prototype
const proto = el instanceof HTMLTextAreaElement
  ? HTMLTextAreaElement.prototype
  : HTMLInputElement.prototype
const nativeSetter = Object.getOwnPropertyDescriptor(proto, 'value')?.set
if (nativeSetter) {
  nativeSetter.call(el, val)
} else {
  el.value = val
}
// Dispatch input event (React listens for this on the native setter path)
el.dispatchEvent(new Event('input', { bubbles: true }))
el.dispatchEvent(new Event('change', { bubbles: true }))
```

This pattern is used by React Testing Library, Playwright, and every serious browser
automation tool. Without it, fill operations silently fail on React/Vue/Angular sites.

### Impact
**All form fills on React/Vue/Angular sites are broken.** The value appears to change
visually but the framework doesn't pick up the change, so:
- Form validation doesn't trigger
- Submit sends the old value
- Controlled components revert to their previous state on re-render

---

## 15. IMPORTANT: CDP Debugger Resilience

### Current Nebo approach
```typescript
const attachedTabs = new Set<number>()

async function ensureDebuggerAttached(tabId: number): Promise<void> {
  if (attachedTabs.has(tabId)) return  // trust the set
  try {
    await chrome.debugger.attach({ tabId }, '1.3')
    attachedTabs.add(tabId)
  } catch (err) {
    if (err.message.includes('Already attached')) {
      attachedTabs.add(tabId)
    } else {
      throw err  // gives up
    }
  }
}
```

### Problem
The `attachedTabs` set can become stale. The debugger can detach for many reasons:
- Page navigation to a different origin
- Browser DevTools opened by user
- Tab crash/reload
- "Cannot attach to this target" for chrome:// or extension pages

The `onDetach` listener removes from the set, but if the service worker was suspended
and restarted (common in MV3), the set is empty while debugger is still attached.

### Fix required
1. **Always try to attach, handle "Already attached" gracefully** — don't trust the set
2. **Re-attach on failure before giving up** — if a CDP command fails with a detach
   error, try to re-attach once and retry the command
3. **Clear the set on service worker startup** — it's stale anyway after suspension

```typescript
async function ensureDebuggerAttached(tabId: number): Promise<void> {
  try {
    await chrome.debugger.attach({ tabId }, '1.3')
    attachedTabs.add(tabId)
  } catch (err) {
    if (err instanceof Error && err.message.includes('Already attached')) {
      attachedTabs.add(tabId)
    } else {
      throw err
    }
  }
}

// Wrapper for CDP commands with auto-reattach
async function cdpCommand(tabId: number, method: string, params?: object): Promise<any> {
  await ensureDebuggerAttached(tabId)
  try {
    return await chrome.debugger.sendCommand({ tabId }, method, params)
  } catch (err) {
    // If detached, try re-attaching once
    if (err instanceof Error &&
        (err.message.includes('not attached') ||
         err.message.includes('Cannot access'))) {
      attachedTabs.delete(tabId)
      await ensureDebuggerAttached(tabId)
      return await chrome.debugger.sendCommand({ tabId }, method, params)
    }
    throw err
  }
}
```

---

## 16. MODERATE: Visual Indicator Tab Group Support

### Claude's extras
- **Static indicator**: Shows "Claude is active in this tab group" on secondary tabs
  in the same tab group, with "Open chat" and "Dismiss" buttons
- **5-second heartbeat**: Polls to verify main tab is still alive, removes indicator
  if main tab died
- **MCP awareness**: Tracks if indicators were triggered by MCP (hides stop button)
- **Separate hide-for-tool-use tracking**: Tracks both glow AND static indicator
  visibility independently

### Nebo's approach
- Glow + stop button only, no tab group concept
- No heartbeat
- No static indicator for other tabs

### Fix required (later)
Not critical for core functionality. Add when tab group management is needed.

---

## 17. MODERATE: Structural Element Detection

### Claude
Separate `isStructural()` check:
```javascript
function p(e) {
  var t = e.tagName.toLowerCase();
  return ["h1","h2","h3","h4","h5","h6","nav","main","header","footer",
          "section","article","aside"].includes(t)
    || null !== e.getAttribute("role");
}
```

### Nebo
No separate structural check — rolled into the confusing `skip` variable logic.

### Fix required
Add explicit `isStructural()` function matching Claude's. Use it in the inclusion logic.

---

## Summary: Priority-Ordered Fix List

### P0 — Blocks core functionality
1. **Element reference stability** (#1) — don't reset map/counter between reads
2. **RefId focused subtree** (#10) — currently completely broken
3. **Select option enumeration** (#2) — agent can't work with dropdowns
4. **Page settle / hydration** (#12) — poll readyState + getAnimations, not fixed 500ms
5. **Text escaping** (#13) — quotes and newlines break tree parsing
6. **React controlled input fill** (#14) — use native value setter for framework compat
7. **CDP debugger resilience** (#15) — auto-reattach on failure, retry commands

### P1 — Major reliability issues
8. **Visibility filtering by mode** (#4) — `all` mode should include everything
9. **Character limit handling** (#9) — generate full tree, return descriptive error
10. **Accessible name: direct text nodes** (#3) — avoid garbage from deep textContent
11. **Accessible name: select option text** (#3) — show selected value as name
12. **Indentation depth** (#11) — only increment when parent was included
13. **Element inclusion logic** (#5) — match Claude's clean shouldInclude pattern

### P2 — Quality improvements
14. **Default role** (#7) — change `""` to `"generic"`
15. **Image role** (#7) — change `img:"img"` to `img:"image"`
16. **Add type/placeholder attributes** (#8) — to tree output
17. **Structural element detection** (#17) — add explicit isStructural() function

### P3 — Nice to have
18. **Static tab group indicator** (#16) — for multi-tab agent sessions
19. **Heartbeat for indicator liveness** (#16) — verify main tab is alive
