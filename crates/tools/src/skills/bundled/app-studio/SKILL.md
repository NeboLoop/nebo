---
name: app-studio
description: "App Studio: build any Nebo app or game, from a simple tracker to a rich designed game. An app is an employee with a page in its own window, on desktop and in the mobile app. Use when the owner asks for an app, a game, a dashboard, a tracker, a form, a viewer, a showcase, or any screen they want to open, and when they ask to change, fix, rename or publish an app."
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
  version: "0.2.2"
---

# App Studio

An app is an employee with a page. The page is the employee's `ui/` folder,
and **only** that folder: Nebo serves it at three addresses at once.

| Where it opens | Address of `ui/index.html` |
|---|---|
| Desktop app window | `neboapp://<id>/index.html` |
| Browser, and `app_screenshot` | `http://127.0.0.1:27895/apps/<id>/ui/index.html` (with `/k/<pass>` in front for a screenshot) |
| Mobile app and web, through the tunnel | `https://neboai.com/t/<bot>/apps/<id>/ui/index.html` |

So **every path the page uses is relative** (`./app.js`, `./assets/hero.webp`).
A path that starts with `/` works in the desktop window and breaks
everywhere else: it leaves `ui/` and gets Nebo's own page or a 404.

Steps 0 to 4 are the mechanics every app follows, in order. Every step ends
with a check; never tell the owner it works until the check passes. When
looks matter, Design Depth adds the studio method on top.

## Hard Rules

1. **Rename, never delete and recreate.** `update_employee(name: "Tracker", new_name: "Deal Board")`
   keeps the id, folder, files, chat and data. Deleting an employee destroys its
   source files for good.
2. **Call tools; never write a tool call as text.** No XML, no JSON in the reply,
   no "tools off". If a tool is not in your list, load it (rule 3).
3. **`app_status`, `app_reload`, `app_console`, `app_screenshot`, `generate_media`,
   `app_data` and `decide` are tools, never shell commands.** Load them once with
   `find_tools(query: "select:app_status,app_reload,app_console,app_screenshot,generate_media,code")`
   at the start of every build or fix: `code` is how you read and check code.
   A tool missing from your list is not loaded yet; it is never "not available".
4. **Never install a runtime.** No `brew install`, no `curl ... | bash`, no bun or
   node downloads (Step 2 says what each lane needs).
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
10. **Small files, starting from the layout in Step 3A.** Each file under about 12 KB,
    one concern per file, at most three files per `update_employee` call. Never
    one big file: it can't be written in one call and every fix rewrites it.
11. **Never bust the cache by hand** (`?v=2`, renaming files). `app_reload` is the reload.
12. **Never bring back code the owner rejected.** Restoring a version he asks
    for, or the last good one after a bad change, is fine (rule 13).
13. **Fix the smallest thing that's broken.** Never rewrite working code to fix
    a bug. Before any large change, say what you'll change and why. The app's
    history is saved automatically: if a change makes things worse, restore the
    last good version instead of rewriting.
14. **No emoji in the interface.** Icons are `lucide-react` (one stroke weight,
    the app's colors) or the app's generated set. Write characters as
    themselves (é, —, ✓), never `\u` escapes: in page text they show as typed.

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
- **From a design** ("make this design an app"): always a new app
  employee, built by you in this chat; never ask, never become it. Follow
  `references/from-a-design.md`.

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
  `orientation` (`portrait` default, `landscape`, `any`), `pull_to_refresh`,
  `voice`, `open_on_work`, `isolated`, `share_menu` and nothing else.
- Off unless set (never on fullscreen): `pull_to_refresh: true` (the mobile app's
  pull-down reload, never on a canvas); `voice: true` (the chat's dictate
  and voice buttons in the mobile app's bar, for an app run by talking).
- `isolated: true` only for a threaded engine export (games.md).
- `share_menu`: the header's Share button, up to 6 `{label, say}`, ways to
  share or export the work, each `say` sent to its chat (link: sdk-more.md).
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

`../../../sdk/nebo.global.js` reaches the SDK from `ui/index.html` at all three
addresses: copy it as written. The absolute `/sdk/nebo.global.js` loads on the
desktop and fails in the mobile app.

**Check 1.** Run `app_status`. It names the folder the page is served from
(`<data>/user/agents/<folder>/ui`); the app folder is the one above it. Write
both down. Then run Verify (Step 4): the screenshot shows "ready: <name>".

Loading this skill turns App Developer mode on: developer tools for the
owner's own apps, never marketplace ones.

## Step 2: Pick the lane, once

| Lane | When | Needs |
|---|---|---|
| **A. Nebo compiles** (default) | Every app: TypeScript, TSX, JSX or plain JS, React and Three.js included | Nothing. Nebo's built-in compiler turns each `.ts`, `.tsx` and `.jsx` file into the `.js` the page loads; npm packages are saved into the app on write and work offline. |
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

Start from this layout; change it when the app needs to (a game, a 3D scene):

```
ui/index.html          the SDK tag and ./app.js, nothing else (Tailwind
                       pages add its tag and config; saved offline on write)
ui/app.jsx             the shell: layout, navigation, which screen shows
ui/store.js            every storage read and write, in one place
ui/screens/<name>.jsx  one screen each (home.jsx, booking.jsx, ...)
ui/parts/<name>.jsx    pieces two screens share (card.jsx, nav.jsx)
ui/style.css           all styles
```

Build order: `index.html`, `app.jsx` and `store.js` first (the page loads),
then one screen per call, `app_reload` after each. A game keeps the same idea:
`app.js` shell, then `scene`, `input`, `audio`, `config` modules.

- Your own modules: `import { startScene } from './scene'` (or `./scene.tsx`);
  it becomes `./scene.js`. Always start with `./`.
- npm packages: a bare import, pinned with `@version`:
  `import * as THREE from 'three@0.170.0'`. `react` and `react-dom` are pinned
  for you (18.3.1); JSX needs no `import React`. The write saves each package
  into `ui/vendor/` (it then loads offline) and pins its version in
  `src/vendor.lock.json`; offline, the result says it stays on esm.sh.
- Types are stripped, not checked. CSS is linked from `index.html`, never imported.
- A local `import('./x')` loads only a file you also sent; never code-split.
- A file that does not compile writes nothing, and the result names it as
  `file:line:column` with the reason. Fix that file and send the call again.

A worked example (an `index.html`, an `app.tsx` with React and storage, a
Three.js `scene.ts`): `references/lane-a-example.md`. Read it before the first
Lane A app.

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

The full Vite lane (project layout, `vite.config` with `base: './'`, building
into `ui/`, what is served): `read_skill_file(name: "app-studio", path:
"references/vite-build.md")`. Read it before the first Vite build.

## Step 4: Verify (after every change)

The fix loop, in this order, every time:

1. Find the code with `code(action: "outline" | "definition" | "references")`,
   not by reading whole files.
2. After each edit: `code(action: "parse_check")` and `"diagnostics"` on every
   file you touched; fix what they list before building.
3. Build (Lane A: `update_employee` compiles; Lane B: the build command).
   A build error is the result: read it, fix it, build again. Never go on
   past a failed build.
4. `app_reload`. It says whether a view was open to reload.
5. `app_console`, and `app_screenshot(width: 390, height: 844)` (add
   1280x800 for a desktop app). The screenshot loads the page the way the
   mobile app does, says what it shows, and ends with that load's console, word
   for word. `app_status` when a file is MISSING or "(outside the app)".
   A game is proved in play: screenshot `index.html?play=1` (it skips the
   start screen), never the menu; for 3D, see `references/games.md` 7a.
6. Report to the owner only when the console has no errors AND the
   screenshot shows the change. Otherwise fix and loop: every known failure (blank page, `nebo is not
   defined`, a build that serves nothing) has its fix in
   `references/when-it-breaks.md`.
   Never say it changed because the build ran.

Only then tell the owner it is ready, in product words: "Your Orbit app is
ready. Open it from your workforce."

## Your App's Data

The page's `storage` and the employee's `app_data` tool are one store: same
keys, same values. What the owner tells one, the other sees.

```
app_data(action: "set",   key: "contacts", value: [{ "name": "John Smith", "phone": "+1 555 0100" }])
app_data(action: "get",   key: "contacts")
```

- Only the app's own employee has the tool, for its own app. A coworker asks it.
- After a `set` or `delete` every open view hears it: `storage.onChange(() => load())`.
- One key holding a list (`contacts`), or one key per record (`contact:<id>`).
  Write the keys into the employee's instructions.
- `value` is JSON itself (an object, a list), never JSON inside a string.

Every action (`query`, `list`, `delete`, `path`, `replace`) and its rules, and
an app that keeps one piece of work per chat (`chat:` keys, `?thread=`,
sealed memory): read `references/app-data.md` before the employee's first
`app_data` call beyond set and get.

### Its personality

An employee with no soul speaks with Nebo's default personality: warm, quick,
fun; it says what it will do, gives short real updates, and says what it
did. Leave it unless the owner asks for another character; a soul
replaces it entirely. Never write "how you talk" rules into AGENT.md that
fight it ("say nothing while you work"): the owner hears silence.

## The SDK Global

This table is the SDK contract. The page reads the SDK from `NeboAppSDK`.
`NeboAppSDK.nebo` (a `NeboAppSDK.NeboSDK`) holds every module, each also at
the top level (`NeboAppSDK.identity` is `NeboAppSDK.nebo.identity`), but two
are renamed there so they don't shadow the browser's: `nebo.fetch` is
`NeboAppSDK.neboFetch`, `nebo.WebSocket` is `NeboAppSDK.NeboWebSocket`.
The full top-level list is `nebo`, `identity`, `storage`, `agents`,
`janus`, `decide`, `share`, `surfaces`, `chat`, `a2ui`, `neboFetch`, `NeboWebSocket`,
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
| `janus.complete({messages, model?, temperature?, max_tokens?, system?}): Promise<string>` | A raw model call: no persona, memory or tools. |
| `decide({state, questions}): Promise<{model, answers, usage}>` | Typed decisions in one fast call (see Typed Decisions). Throws with the reason on 400 (malformed), 429 (no work left on the account), 503 (NeboAI not connected). |
| `nebo.fetch(pathOrUrl, init?)` — top level `NeboAppSDK.neboFetch` | A relative path goes to the app's sidecar API; `http(s)://` goes through Nebo's proxy (needs `network:<host>`). |
| `new nebo.WebSocket()` — top level `new NeboAppSDK.NeboWebSocket()` | Live socket to the app's employee; reconnects. `send`, `close`, `onopen/onmessage/onerror/onclose`. |

Streaming (`agents.stream`, `janus.stream`), cards from the employee
(`surfaces`), Nebo's chat inside the page (`chat.mount`) and pages served
outside Nebo (`nebo.configure`): `references/sdk-more.md`.

The SDK finds the app and the address prefix itself; a page never sets them.
The starter page checks the wiring: `NeboAppSDK.nebo.identity.get()` prints
the employee's name.

## Version History

Every turn that changes the app is saved. `app_status(history: true)` lists
the versions; `app_reload(restore: "<id>")` puts the page and source back (a
restore is a version too, so it can be undone).

## Design Depth (the studio method)

When looks matter (a game, a showcase, anything shown to other people), read
`references/design-depth.md` before the brief and follow it: brief, boards,
assets, build, motion, gate.

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

## Typed Decisions

When the app makes a judgment (is this lead hot, which category, how urgent, a
game's opponent), ask a typed decision instead of a model call: the page's
`decide`, the employee's `decide(state:, questions:)` tool. Read
`references/decisions.md` before writing the first one.

## Art and Media

`generate_media(kind: "image" | "video", prompt, into: "assets/hero.webp")` writes
into `ui/` (pass `app` for another app's); use `./assets/hero.webp`. Video plays
as `<video muted playsinline autoplay loop>`. 100 MB a file, 500 MB in all.
Motion post or trailer: `references/motion.md`.

## Games and Full-Screen Pages

A game, or any page that takes the whole screen: read `references/games.md`
before the brief (the window settings, tilt, tuning, the loop, input, audio,
sprites and 3D).

## A Server of Its Own

Before suggesting a sidecar (a native server beside the page), check the
table in `references/sidecars.md`: most needs are covered without one.

## Publish

Only when the owner asks ("publish yourself", or **Publish**): load the
`publish-an-app` skill and follow it (`app_listing`, screenshots,
`app_submit` after the owner's yes).

## Deleting

Only when the owner asks to remove the app for good: `delete_employee(name: "<name>")`.
Deleting the folder by hand only deactivates the employee. Never delete to rename.

## Vocabulary

Say employee and hire; say the owner. An app is "your <name> app", never a bot
or an assistant.
