---
name: app-studio
description: "App Studio: build any Nebo app or game, from a simple tracker to a rich designed game. An app is an employee with a page in its own window, on desktop and phone. Use when the owner asks for an app, a game, a dashboard, a tracker, a form, a viewer, a showcase, or any screen they want to open, and when they ask to change, fix, rename or publish an app."
triggers:
  - make an app
  - build an app
  - create an app
  - make a game
  - build a game
  - dashboard
  - app interface
  - a page I can open
  - a screen for
  - tracker
  - a form for
  - fix the app
  - make it stunning
  - landing app
  - showcase app
  - interactive story
  - app studio
metadata:
  version: "0.2.1"
---

# App Studio

An app is an employee with a page. The page is the employee's `ui/` folder,
and **only** that folder: Nebo serves it at three addresses at once.

| Where it opens | Address of `ui/index.html` |
|---|---|
| Desktop app window | `neboapp://<id>/index.html` |
| Browser, and `app_screenshot` | `http://127.0.0.1:27895/apps/<id>/ui/index.html` (with `/k/<pass>` in front for a screenshot) |
| Phone and web, through the tunnel | `https://neboai.com/t/<bot>/apps/<id>/ui/index.html` |

So **every path the page uses is relative** (`./app.js`, `./assets/hero.webp`).
A path that starts with `/` works in the desktop window and breaks
everywhere else: it leaves `ui/` and gets Nebo's own page or a 404.

Steps 0 to 4 are the mechanics every app follows, in order. Every step ends
with a check; never tell the owner it works until the check passes. When
looks matter (a game, a showcase, anything shown to other people), the
Design Depth section adds the studio method on top, from this skill's
`references/`.

## Hard Rules

1. **Rename, never delete and recreate.** `update_employee(name: "Tweet", new_name: "Flip-Flap")`
   keeps the id, folder, files, chat and data. Deleting an employee destroys its
   source files for good.
2. **Call tools; never write a tool call as text.** No XML, no JSON in the reply,
   no "tools off". If a tool is not in your list, load it (rule 3).
3. **`app_status`, `app_reload`, `app_console`, `app_screenshot`, `generate_media`,
   `app_data` and `decide` are tools, never shell commands.** Load them once with
   `find_tools(query: "select:app_status,app_reload,app_console,app_screenshot,generate_media")`.
   A tool missing from your list is not loaded yet; it is never "not available".
4. **Never install a runtime.** No `brew install`, no `curl ... | bash`, no bun or
   node downloads. Lane A needs nothing; Lane B runs only on a bot that already
   has node (Step 2).
5. **`ui/` holds what the page serves and nothing else.** No `package.json`,
   `src/`, `node_modules/` or `dist/` inside `ui/`. A build writes only into `ui/`.
6. **No `index.html` in the app folder itself.** The only entry is `ui/index.html`;
   a build's source entry is `src/index.html`.
7. **Nothing in the page starts with `/`.** Not scripts, styles, images, `fetch`
   paths, or a bundler's `base`. The SDK tag below is relative too.
8. **One writer per app.** Only the session building the app edits its files.
   Errors the owner pastes, or sends with the console's "Send to <app>" button,
   are fixed in that same session, in the same files. Never start a second copy.
9. **Never code around a missing SDK** (no `localStorage` fallback, no guard that
   skips the game). `NeboAppSDK` missing means the script tag is wrong: fix the tag.
10. **Files under 20 KB each.** A bigger single write gets cut off. Split the page
    into modules (`app.tsx`, `board.tsx`, `scene.ts`).
11. **Never bust the cache by hand** (`?v=2`, renaming files). `app_reload` is the reload.
12. **Never bring back code the owner rejected.** Restoring a version he asks
    for, or the last good one after a bad change, is fine (rule 13).
13. **Fix the smallest thing that's broken.** Never rewrite working code to fix
    a bug. Before any large change, say what you'll change and why. The app's
    history is saved automatically: if a change makes things worse, restore the
    last good version instead of rewriting.

## Step 0: Whose app is it?

An app can be built two ways. Pick by what the owner says:

- **You become the app** when they put it on you: "you are the app", "let's
  build you", "make yourself a game". One update gives you a page and makes
  you an app; you stay yourself, with your chat, memory and persona, the
  owner can talk to you while the page reloads, and you can publish yourself.
- **A new app employee** when they ask for an app as a separate thing ("make
  me an app for my orders") or it is another employee's job.
- **Not clear which?** Ask once, in your first reply: "Should I become this
  app, or build it as a new one?" When the owner says "you", act at once:
  never ask again and never create another employee.

## Step 1: Make the app with a starter page

**Becoming the app** (one call, with your own name):

```
update_employee(
  name: "<your own name>",
  app: { window: { title: "Solitaire", fullscreen: true, orientation: "landscape" } },
  ui: { "index.html": "<the starter page below>" }
)
```

**A new app employee** takes two calls. The first drafts; the second, with
only the `draft_id` it returns, creates it (at once when the owner already
told you to make it, else after their yes):

```
create_employee(
  name: "Deal Board",
  description: "A board of open deals, sorted by close date.",
  app: { window: { title: "Deal Board", width: 900, height: 700 }, permissions: ["storage:readwrite"] },
  ui: { "index.html": "<the starter page below>" }
)
create_employee(draft_id: "<the id it returned>")
```

- `app` and `ui` are objects, never JSON strings. Content never goes in `draft_id`.
- `name` is the employee and its folder. It is NOT
  the app id: the id is a UUID, minted at create, given in the result and by
  `get_employee(name: "Deal Board")`.
- `window` takes `title`, `width`, `height`, `resizable`, `fullscreen`,
  `orientation` (`portrait` default, `landscape`, `any`), `pull_to_refresh`
  and nothing else.
- `pull_to_refresh: false` turns off the phone's pull-down-to-reload for a page
  where dragging down is play (dragging cards in a solitaire game) without
  making it fullscreen. Fullscreen apps never have it.
- `permissions`: `storage:readwrite`, `subagent:<employee-id>`, `network:<host>`
  or `network:*` (the proxy fetch), `device:motion` (tilt).

The starter page, exactly:

```html
<!doctype html>
<html>
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
  <title>Deal Board</title>
</head>
<body>
  <pre id="who">loading...</pre>
  <script src="../../../sdk/nebo.global.js"></script>
  <script>
    NeboAppSDK.nebo.identity.get()
      .then(me => { document.getElementById('who').textContent = 'ready: ' + me.name; console.log('ready', me.name); })
      .catch(err => { document.getElementById('who').textContent = String(err); console.error(err); });
  </script>
</body>
</html>
```

`../../../sdk/nebo.global.js` is the SDK's address from `ui/index.html` at all
three addresses (it resolves to `/sdk/nebo.global.js`, `/t/<bot>/sdk/nebo.global.js`
and `neboapp://<id>/sdk/nebo.global.js`). Copy it as written. The absolute
`/sdk/nebo.global.js` loads on the desktop and fails on the phone.

**Check 1.** Run `app_status`. It names the folder the page is served from
(`<data>/user/agents/<folder>/ui`); the app folder is the one above it. Write
both down. Then run Verify (Step 4): the screenshot shows "ready: <name>".

Loading this skill turns App Developer mode on (the load says so the first
time): the developer tools then work on any of the owner's own apps, pages
are served uncached, and each page carries the floating console. An app
installed from the marketplace is never built here.

## Step 2: Pick the lane, once

| Lane | When | Needs |
|---|---|---|
| **A. Nebo compiles** (default) | Every app: TypeScript, TSX, JSX or plain JS, React and Three.js included | Nothing. Nebo's built-in compiler turns each `.ts`, `.tsx` and `.jsx` file into the `.js` the page loads; npm packages load from esm.sh at run time. |
| **B. Vite build** | The owner asks for a Vite project, or the app needs a package that does not load from esm.sh | `node --version` succeeds on this bot (cloud bots have node and npm; a desktop may not). |

Check with `run_command("node --version")` only when Lane B is asked for.
No node: Lane A, and tell the owner in one line that the app needs no build
tools. Never switch lanes mid-app without saying so.

## Step 3A: Nebo compiles

Write the page as TypeScript or JSX modules and send every file through the
one tool. `update_employee` compiles each `.ts`, `.tsx` and `.jsx` file in
`ui` to a `.js` file of the same name in `ui/` (`app.tsx` → `app.js`) and
keeps your source in the app folder's `src/` (`src/app.tsx`). The page loads
the `.js` names.

- Your own modules: `import { startScene } from './scene'` (or `./scene.tsx`);
  it becomes `./scene.js`. Always start with `./`.
- npm packages: a bare import, pinned with `@version`:
  `import * as THREE from 'three@0.170.0'`. `react` and `react-dom` are pinned
  for you (18.3.1); JSX needs no `import React`.
- Types are stripped, not checked. CSS is linked from `index.html`, never imported.
- A file that does not compile writes nothing, and the result names it as
  `file:line:column` with the reason. Fix that file and send the call again.

`ui/index.html` (plain HTML, written as is):

```html
<!doctype html>
<html>
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
  <title>Orbit</title>
  <link rel="stylesheet" href="./style.css">
</head>
<body>
  <div id="root"></div>
  <script src="../../../sdk/nebo.global.js"></script>
  <script type="module" src="./app.js"></script>
</body>
</html>
```

`ui/app.tsx`:

```tsx
import { useEffect, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { startScene } from './scene';

const { nebo } = (window as any).NeboAppSDK;

function App() {
  const canvas = useRef<HTMLCanvasElement>(null);
  const [best, setBest] = useState<number>(0);
  useEffect(() => { nebo.storage.getItem('best').then((v: number | null) => setBest(v ?? 0)); }, []);
  useEffect(() => startScene(canvas.current!), []);
  return <main><canvas ref={canvas} /><p>Best: {best}</p></main>;
}
createRoot(document.getElementById('root')!).render(<App />);
console.log('ready');
```

`ui/scene.ts` starts with `import * as THREE from 'three@0.170.0';` and
exports `startScene(canvas): () => void` (the cleanup).

Send them all in one call:

```
update_employee(name: "Orbit", ui: { "index.html": "...", "app.tsx": "...", "scene.ts": "...", "style.css": "..." })
```

Never edit the compiled `.js` in `ui/` (the next send overwrites it), and
never only `edit_file` a file in `src/`: nothing reaches the page until the
whole changed file is sent through `update_employee` again. Plain `.js`
modules work too; they are written as is, so give their npm imports full
`https://esm.sh/<package>@<version>` addresses.

A single small React component can instead go in `ui_jsx` (one file with
`export default`, made into `ui/index.html`).

## Step 3B: Vite build

Everything lives in the app folder from Check 1. Run each command with
`cd "<app folder>" &&` in front. These use npm because it is what the bot has.

```
<app folder>/
├── AGENT.md  manifest.json  agent.json   # the package (update_employee writes these)
├── package.json  vite.config.mjs          # build files, never served
├── node_modules/                          # never served, never published
├── src/
│   ├── index.html                         # the build's entry (not served)
│   └── main.tsx                           # or main.jsx
└── ui/                                    # build output + generated media: what Nebo serves
    ├── index.html                         # written by the build
    ├── bundle/                            # written by the build, replaced every build
    └── assets/                            # generate_media output, kept across builds
```

1. Set up once:

   ```bash
   npm init -y
   npm install --save-dev vite @vitejs/plugin-react
   npm install react react-dom three
   ```

2. Write `vite.config.mjs` exactly (write_file, in the app folder):

   ```js
   import { defineConfig } from 'vite';
   import react from '@vitejs/plugin-react';

   export default defineConfig({
     root: 'src',
     base: './',
     plugins: [react()],
     publicDir: false,
     build: { outDir: '../ui', emptyOutDir: false, assetsDir: 'bundle' },
   });
   ```

   - `base: './'` makes every built path relative (rule 7).
   - `outDir: '../ui'` is relative to `root`; it is the app's `ui/`.
   - `emptyOutDir: false` keeps `ui/assets/` (your generated media) alive;
     the build command below clears only `ui/bundle/`.

3. Write `src/index.html` with the SDK tag and the entry, both relative:

   ```html
   <!doctype html>
   <html>
   <head>
     <meta charset="utf-8">
     <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
     <title>Orbit</title>
   </head>
   <body>
     <div id="root"></div>
     <script src="../../../sdk/nebo.global.js"></script>
     <script type="module" src="./main.tsx"></script>
   </body>
   </html>
   ```

   Vite compiles TypeScript and JSX itself (no tsconfig needed), leaves
   the SDK tag alone (it is a classic script) and rewrites the module entry to `./bundle/index-<hash>.js`. Read the SDK at run time,
   `const { nebo } = window.NeboAppSDK;`; never install a package for it.
   Media from `generate_media` is referenced as `./assets/hero.webp`.

4. Build, every time:

   ```bash
   rm -rf ui/bundle && npx vite build
   ```

   Two warnings are expected and harmless: the SDK tag "can't be bundled
   without type=module", and chunks "larger than 500 kB".

5. **Check 3B.** `ui/index.html` names `./bundle/index-<hash>.js` and
   `../../../sdk/nebo.global.js`, and nothing that starts with `/`:

   ```bash
   grep -o '\(src\|href\)="[^"]*"' ui/index.html
   ```

Never hand-edit the built `ui/index.html` and never copy build output over
`src/`: change `src/`, rebuild. `crossorigin` and `type="module"` on the
built tags are correct; leave them.

## Step 4: Verify (after every change)

The fix loop, in this order, every time:

1. Find the code with `code`, not by reading whole files:
   `code(action: "outline", path)`, then `definition` / `references` at a
   line and column to reach what to change.
2. After each edit: `code(action: "parse_check", path)`, then
   `code(action: "diagnostics", path)` on every file you touched. Fix
   what they list before building. (A `.ts`/`.tsx` file with no language
   server is checked by the project's own `tsc`.)
3. Build (Lane A: `update_employee` compiles; Lane B: the build command).
   A build error is the result: read it, fix it, build again. Never go on
   past a failed build.
4. `app_reload`. It says whether a view was open to reload.
5. `app_console`, and `app_screenshot(width: 390, height: 844)` (add
   1280x800 for a desktop app). The screenshot loads the page the way the
   phone does, says what it shows, and ends with that load's console, word
   for word. `app_status` when a file is MISSING or "(outside the app)".
   A game is proved in play: screenshot `index.html?play=1` (it skips the
   start screen), never the menu; for 3D, see `references/games.md` 7a.
6. Report to the owner only when the console has no errors AND the
   screenshot shows the change. Otherwise fix (the table below) and loop.
   Never say it changed because the build ran.

Only then tell the owner it is ready, in product words: "Your Orbit app is
ready. Open it from your workforce."

## When It Breaks

One loop for every fix:

1. Reproduce: `app_screenshot`.
2. Read its console and the screenshot (`app_console` for more).
3. Make the minimal change to the one file at fault.
4. `app_reload`, then Verify (Step 4).
5. Still broken, or worse: restore the last good version, then try a
   different small change.

| Symptom | Cause | Fix |
|---|---|---|
| Blank page; console: `Failed to load /assets/index-....js`, or a module "MIME type text/html" error | A path starts with `/`; it left `ui/` and got Nebo's own page | Make it relative. Vite: `base: './'`, rebuild |
| Works in the desktop window, blank on the phone | Same: the desktop window serves `ui/` at its root, the phone serves it under `/t/<bot>/apps/<id>/ui/` | Same |
| `NeboAppSDK is not defined`, `Cannot destructure property 'nebo' of null`, `Failed to load /sdk/nebo.global.js` | The SDK tag is absolute, missing, or after your module | `<script src="../../../sdk/nebo.global.js"></script>` before your scripts. Never add a fallback |
| `nebo is not defined` | There is no bare `nebo` global | `const { nebo } = window.NeboAppSDK;` |
| `Failed to load /src/main.jsx`, or the page shows the build's source | The source entry is being served: `index.html` in the app folder or in `ui/` points at `src/` | Entry source lives in `src/index.html`; serve only built `ui/index.html`; rebuild |
| `app_status` lists `node_modules/`, `package.json` or `src/` under `ui/` | The project was set up inside `ui/` | Move the project to the app folder (Step 3B), delete those from `ui/`, rebuild |
| A change does not show | Wrong folder, the build did not run, or no reload | `app_status` (folder, file times), rebuild, `app_reload` |
| `command not found: app_status` | Developer tools run as shell commands | They are tools: `find_tools(query: "select:app_status,app_reload,app_console,app_screenshot")` |
| `There is no tool named generate_media` | Not loaded yet | `find_tools(query: "select:generate_media")`, then call it. Never fall back to no art because of this |
| `bun: command not found`, `npm: command not found` | No runtime on this bot | Lane A. Never install one |
| `update_employee` says `app.tsx:12:7 parse error ... Nothing was written` | That file does not compile | Fix that line, send the whole file again |
| An edit to `src/app.tsx` does not show | `src/` is the kept source; only `update_employee` compiles | Send the changed file through `update_employee(ui: {...})` |
| `a page module cannot import a stylesheet` | `import './app.css'` in Lane A | `<link rel="stylesheet" href="./app.css">` in `index.html` |
| A write ends "cut off at the output limit" | One file too big | Split it into modules under 20 KB |
| `update_employee needs name` | The call had no `name` | Pass your own or the app's name; `ui` is an object of path to text |
| Two sessions keep overwriting each other | Two writers on one app | One writer: answer pasted errors in the building session |
| The owner wants a new name | A rename, not a new app | `update_employee(name: "<old>", new_name: "<new>")`. Never delete |
| `app_console` shows nothing at all | No view has loaded the page since the change | `app_screenshot`, then `app_console` again |
| A change made it worse, or the owner wants it back ("how it was this morning") | | `app_status(history: true)`, pick the version by time, `app_reload(restore: "<id>")`. Never rewrite from memory |

## Version History

Nebo saves the app folder (`ui/`, `src/`, build files; never `node_modules/`
or `dist/`) before and after every turn that changes it. `app_status(history: true)`
lists the versions: id, time, what changed. `app_reload(restore: "<id>")` puts
the page and source back and reloads; the employee's settings stay. A restore
is a new version, so restoring the version before it undoes it. A renamed app
keeps its history; a deleted one keeps it in the trash.

## Design Depth (the studio method)

For a game, a showcase, a landing app or anything the owner will show other
people, run these phases on top of Steps 0 to 4. A plain internal tool (a
tracker, a form) skips this section. Read each reference with
`read_skill_file(name: "app-studio", path: "references/<file>")` when its
phase starts, not before. A small edit to an app that already went through
the method (copy, one component, a color) does not restart it: edit,
rebuild, Verify, gate.

| # | Phase | Leaves | Reference |
|---|-------|--------|-----------|
| 0 | Intake: ONE batched question round (app or game; animated or still, recommend animated; their brand or free rein). No answer: animated, free rein, say so in a line. | the answers | |
| 1 | Brief: `brief.md` in the app folder, six variety axes in front-matter, concept spine, locked palette (hex) and type pair, screen, asset and CTA plans. Differs from every other app's brief on 4 of 6 axes. | `brief.md` | `brief.md`, `design-recipe.md`, `wow-catalog.md` |
| 2 | Boards: one generated image per screen into `boards/`, each looked at once, template-looking ones redone (two redos max). | `ui/boards/*.png` | `boards-and-assets.md` |
| 3 | Assets: every image, film and model submitted at once with the locked hexes; owner's assets win. | `ui/assets/*` | `boards-and-assets.md`, `games.md` for a game |
| 4 | Build each screen to its board (Step 3A or 3B). The board wins over habit. | the page | `design-recipe.md`, `kit.md` |
| 5 | Motion: ONE signature effect that answers the person's input, fully wired, with a `prefers-reduced-motion` fallback. | the effect | `wow-catalog.md`, `film-scrub.md` |
| 6 | Gate: zero failures, then Verify. Delete `ui/boards/` before publish. | a pass | `gate.md` |

**The gate.** On a bot with node, run this skill's checker on the app folder:
`execute(skill: "app-studio", script: "scripts/gate.js", args: { "app": "<app folder>" })`.
It fails on the brief, banned palettes and words, em-dashes, placeholders,
unused or oversize files, missing reduced-motion or touch handling, a
leading `/` in the page, project files inside `ui/`, an `index.html` in the
app folder, the package files, and closeness to the bot's other apps.
Without node, or when it cannot run, check the list in `references/gate.md`
by hand. A design-depth app with a failing gate is not done.

**Banned defaults** (the model's own habits): near-black plus orange, amber,
or neon cyan, blue or green; purple glow; beige plus brass, clay or
oxblood (unless the owner's brand names them); Inter as the display face;
three equal cards in a row; a fake product UI built from divs; em-dashes in
visible text; Elevate, Seamless, Unleash, Next-Gen, Revolutionize; invented
stats; "Jane Doe" testimonials; fade-ins and marquees as the signature
effect.

**No art tool.** Load `generate_media` with `find_tools` first. Only when
`find_tools` does not find it, write each board in words inside the brief,
record `mode: no-generation`, and make the art by hand (SVG, canvas, shaders).

**Turn economy.** Write each file once, complete. Submit independent
generations together. Look at each generated image once.

**Talking to the owner.** Product words: "Designing the screens", "Making
the art", "Your app is ready, open it from your workforce". Never narrate
bundlers, hashes or folders unless asked. At the end, list what the owner
now owns (logo, icons, art, film) and anything honestly skipped.

The method's design parts are adapted from an MIT-licensed work; the notice
is in this skill's `LICENSE-THIRD-PARTY.txt`.

## Iterate

Change a Lane A app the way you made it, through update_employee. Each path
in `ui` overwrites that one file (a `.tsx` its compiled `.js`) and leaves the
rest of the folder alone; `app` changes only the fields it carries;
`agent_md` rewrites the persona. Read the current source from the app
folder's `src/` first, then send the whole changed file:

```
update_employee(name: "Deal Board", ui: { "app.tsx": "<the whole new file>" })
```

A Lane B app changes in `src/`, then the build command, then Verify. The
package (manifest, permissions, persona) changes only through update_employee
in both lanes. The page is read from disk on every request; a manifest or
persona change is picked up within seconds. Nothing to restart.

Keep state in `storage`, not in the page: the window is closed and reopened
and the page reloads on every rebuild.

## The SDK Global

This table is the SDK contract. The page loads the SDK and reads it from
`NeboAppSDK`. `NeboAppSDK.nebo` is the singleton, an instance of
`NeboAppSDK.NeboSDK`; every module is also exported at the top level, so
`NeboAppSDK.identity` and `NeboAppSDK.nebo.identity` are the same object.
Two are named differently at the top level, because a bare `fetch` or
`WebSocket` export would shadow the browser's: `nebo.fetch` is
`NeboAppSDK.neboFetch`, and `nebo.WebSocket` is `NeboAppSDK.NeboWebSocket`.
The full top-level list is `nebo`, `identity`, `storage`, `agents`,
`janus`, `decide`, `surfaces`, `chat`, `a2ui`, `neboFetch`, `NeboWebSocket`,
`NeboSDK`, `NeboSurfaces`, `NeboA2UI`, `getAppId`, `getBaseUrl`, `setAppId`,
`setBaseUrl`. The canonical address is `/sdk/nebo.global.js`; from
`ui/index.html` it is loaded as `../../../sdk/nebo.global.js`.

| Call | Does |
|------|------|
| `identity.get(): Promise<{id, name, displayName, description, persona, model, skills, inputValues}>` | Who this app's employee is. Cached; `identity.invalidate()` clears it. |
| `storage.getItem(key): Promise<any \| null>` | Read one key of the app's store. JSON comes back parsed. |
| `storage.setItem(key, value): Promise<void>` | Write one key. Non-strings are JSON-encoded. |
| `storage.removeItem(key)`, `storage.keys(): Promise<string[]>`, `storage.clear()` | The rest of the store. |
| `storage.onChange(cb): () => void` | `cb({appId, keys, action, source})` after every write: `source` is `"employee"` (`app_data`) or `"page"` (another open view). Returns an unsubscribe. |
| `agents.invoke(message, {agent?, data?}): Promise<{text, tools?}>` | Ask an employee, wait for the answer. `agent` names another employee (needs `subagent:<id>`). |
| `agents.stream(message, {agent?, data?}): AsyncGenerator<{text, done}>` | The same, streamed. |
| `janus.complete({messages, model?, temperature?, max_tokens?, system?}): Promise<string>` | A raw model call: no persona, memory or tools. |
| `janus.stream(same): AsyncGenerator<string>` | The same, streamed. |
| `decide({state, questions}): Promise<{model, answers, usage}>` | Typed decisions in one fast call (see Typed Decisions). Throws with the reason on 400 (malformed), 429 (no work left on the account), 503 (NeboAI not connected). |
| `nebo.fetch(pathOrUrl, init?)` — top level `NeboAppSDK.neboFetch` | A relative path goes to the app's sidecar API; `http(s)://` goes through Nebo's proxy (needs `network:<host>`). |
| `new nebo.WebSocket()` — top level `new NeboAppSDK.NeboWebSocket()` | Live socket to the app's employee; reconnects. `send`, `close`, `onopen/onmessage/onerror/onclose`. |
| `surfaces.connect()`, `surfaces.on(type, handler)`, `surfaces.send(name, payload)`, `surfaces.state` | Cards from the employee's `a2ui` tool. |
| `chat.mount(el, {placeholder?, theme?, height?, borderless?, contextId?, scope?})` | Nebo's chat with this employee, inside the page. `chat.send`, `chat.setContext`, `chat.onMessage`, `chat.newThread`, `chat.unmount` drive it. |
| `nebo.configure({appId?, baseUrl?})` | Only for a page served outside Nebo; at the top level `NeboAppSDK.setAppId(id)` / `NeboAppSDK.setBaseUrl(url)`, read back with `getAppId()` / `getBaseUrl()`. |

The SDK finds the app and the address prefix by itself; a page never sets them.
The check that the wiring is right is the starter page: `NeboAppSDK.nebo.identity.get()`
prints the employee's name.

## Your App's Data

The page's `storage` and the app employee's `app_data` tool are one store:
same keys, same values. What the owner tells the employee shows on the page;
what they type on the page, the employee can find.

```
app_data(action: "set",   key: "contacts", value: [{ "name": "John Smith", "phone": "+1 555 0100" }])
app_data(action: "get",   key: "contacts")
app_data(action: "query", where: { "name": "john smith" })
app_data(action: "query", text: "smith", prefix: "contact:", limit: 5)
app_data(action: "list",  prefix: "contact:")
app_data(action: "delete", key: "draft")
```

- Only the app's own employee has the tool, for its own app. A coworker asks it.
- After a `set` or `delete` every open view hears it: `storage.onChange(() => load())`.
- One key holding a list (`contacts`), or one key per record under a prefix
  (`contact:<id>`). Write the keys into the employee's instructions.

## Typed Decisions

For a judgment (is this lead hot, which category, how urgent), ask a typed
decision instead of a model call. The page uses `decide`; the app's employee
has the `decide` tool with the same request. Offer it when the app makes
choices: a game's opponent, a triage, a score.

```js
const { answers } = await NeboAppSDK.decide({
  state: lead,
  questions: {
    tier: { type: "choice", instructions: "How warm is this lead, by `status` and `last_contact`?",
            criteria: { hot: "ready to buy now", warm: "interested", cold: "no interest", other: "can't tell" } },
    fit:  { type: "score", instructions: "How well does `company` fit our customers?", criteria: ["poor", "fair", "good", "great"] },
    reply:{ type: "noul", instructions: "`message` asks us for a reply." },
  },
});
```

```
decide(state: <records from app_data>, questions: { "tier": { "type": "choice", "instructions": "...", "criteria": { ... } } })
```

The whole question lives in `instructions`, naming the state's fields in
backticks. Include an escape option (`other`). Counting, dates and
thresholds stay in code.

## Art and Media

`generate_media(kind: "image", prompt: "...", into: "assets/hero.webp")` writes
into the app's `ui/` (when you are the app; pass `app` for another). Reference
it as `./assets/hero.webp`. Video: `kind: "video"`, `<video muted playsinline autoplay loop>`.
Each file at most 10 MB; the whole package at most 50 MB.

## Games and Full-Screen Pages

```
app: { window: { title: "Kart", fullscreen: true, orientation: "landscape" },
       permissions: ["storage:readwrite", "device:motion"] }
```

- `fullscreen: true`: no app bar, no safe-area padding, screen stays awake,
  pull-to-refresh off. Pad with `env(safe-area-inset-*)`; keep controls clear of
  the close button in the top-left corner.
- Tilt (`device:motion`): on iPhone call `DeviceMotionEvent.requestPermission()`
  from the first tap, the same tap that starts sound.
- Put every tuning number (speed, turn rate, gravity) in one `config.js` object,
  so a "feels wrong" is one edit, not a rewrite.
- Saves go in `storage`.
- A game's loop, input, audio, sprites and 3D: `references/games.md` (read it
  before the brief). Multiplayer: the page opens its own WebSocket to the
  game server; nothing goes through Nebo.

## Publish

Only when the owner asks ("publish yourself", or **Publish** in the app's
chat, on the phone's app screen, or in the desktop menu). Follow the bundled
`publish-an-app` skill: `app_listing`, then 3 to 5 `app_screenshot(for_listing: true)`,
then `app_submit` after the owner's yes. The bundle is AGENT.md, agent.json,
manifest.json, `ui/` and the app's own `skills/`; never `src/`, `node_modules/`
or build files.

## Deleting

Only when the owner asks to remove the app for good: `delete_employee(name: "<name>")`.
Deleting the folder by hand only deactivates the employee. Never delete to rename.

## Vocabulary

Say employee and hire; say the owner. An app is "your <name> app", never a bot
or an assistant.
