---
name: build-an-app
description: Build an app for the owner — a page with its own window that runs inside Nebo, backed by an employee. Use when the owner asks for an app, a dashboard, a tracker, a form, a viewer, or any screen they want to open and use, and when they ask to change an app that already exists.
triggers:
  - make an app
  - build an app
  - create an app
  - dashboard
  - app interface
  - a page I can open
  - a screen for
  - tracker
  - a form for
---

# Build an App

An app is an employee with a page. The page is plain HTML under the employee's
`ui/` folder; Nebo serves it at `/apps/<id>/ui/` and opens it in its own window.
The page talks to Nebo through one script, `/sdk/nebo.global.js`, which puts a
single global on the page: `NeboAppSDK`. Nothing to install, nothing to build.

---

## When to Use

- "Make me an app that..." / "Build a dashboard for..." / "I want a screen where I can..."
- "Add a tracker for..." / "A form my team fills in..."
- "Change the app" / "Add a button to the dashboard"

Do not use this for a document, a spreadsheet, or a one-off answer. An app is
for something the owner will open again.

---

## Create It in One Call

Create the app employee with the registry door. One call writes the folder,
the manifest, the persona, and the page; the app appears in the owner's
workforce within a few seconds (measured at about 1.5 s) and opens at
`/apps/<id>/ui/`.

```
agent(
  resource: "registry",
  action: "create",
  name: "deal-board",
  description: "A board of open deals, sorted by close date.",
  app: {
    window: { title: "Deal Board", width: 900, height: 700, resizable: true },
    permissions: ["storage:readwrite"]
  },
  ui: {
    "index.html": "<!doctype html>...",
    "app.js": "..."
  }
)
```

Parameters, exactly:

- `name` — the employee's name, and the folder its package lives in. It is NOT
  the app id: the id is a UUID, minted at create. The create result says it
  ("Created agent 'deal-board' (id: ...)"), and
  `agent(resource: "registry", action: "info", name: "deal-board")` says it
  again later. The page is served at `/apps/<that id>/ui/`.
- `description` — one line; it becomes the persona if you do not pass `agent_md`.
- `app` — `{window, permissions}`. Passing it makes the employee an app.
  - `window`: `title`, `width`, `height`, `resizable` — those four and no
    others. Defaults are 1024 x 768, resizable, titled after the employee.
  - `permissions`: strings in `prefix:scope` form. `storage:readwrite` for the
    key-value store, `subagent:<employee-id>` to invoke another employee (your
    own employee needs no permission), `network:<host>` or `network:*` for the
    HTTP proxy.
- `ui` — a map of relative path to file content. `index.html` is the entry;
  every other file lands beside it and is served at `/apps/<id>/ui/<path>`.
- `ui_jsx` — one JSX source instead of `ui`; it is converted to `ui/index.html`.
  Use `ui` when you can write the HTML yourself; it is smaller and it is what
  you will edit later.

On disk this is `<data_dir>/user/agents/<name>/` holding `manifest.json`
(`"artifact_type": "app"`, `window`, `permissions`), `AGENT.md`, and `ui/`.

---

## The SDK Global

This table is the SDK contract — the one place it is written down. The tool
description points here rather than repeating it.

The page loads the SDK and reads it from `NeboAppSDK`. There is no `nebo`
global. `NeboAppSDK.nebo` is the singleton, an instance of `NeboAppSDK.NeboSDK`;
every module is also exported at the top level, so `NeboAppSDK.identity` and
`NeboAppSDK.nebo.identity` are the same object. Two of them are named
differently at the top level than on the instance, because a bare `fetch` or
`WebSocket` export would shadow the browser's: `nebo.fetch` is exported as
`NeboAppSDK.neboFetch`, and `nebo.WebSocket` as `NeboAppSDK.NeboWebSocket`.
The full top-level export list is `nebo`, `identity`, `storage`, `agents`,
`janus`, `surfaces`, `chat`, `a2ui`, `neboFetch`, `NeboWebSocket`, `NeboSDK`,
`NeboSurfaces`, `NeboA2UI`, `getAppId`, `getBaseUrl`, `setAppId`, `setBaseUrl`.

```html
<script src="/sdk/nebo.global.js"></script>
<script>
  const { identity, storage, agents, janus, chat, surfaces } = NeboAppSDK;
</script>
```

The SDK finds the app id from the page URL (`/apps/<id>/ui/...`). A page served
from anywhere else needs `<meta name="nebo-app-id" content="<id>">`.

What it exposes, one line each:

| Call | Does |
|------|------|
| `identity.get(): Promise<{id, name, displayName, description, persona, model, skills, inputValues}>` | Who this app's employee is. Cached; `identity.invalidate()` clears the cache. Answers only for app employees. |
| `storage.getItem(key): Promise<any \| null>` | Read one key of the app's key-value store. JSON values come back parsed. |
| `storage.setItem(key, value): Promise<void>` | Write one key. Non-strings are JSON-encoded. |
| `storage.removeItem(key)`, `storage.keys(): Promise<string[]>`, `storage.clear()` | The rest of the store. |
| `agents.invoke(message, {agent?, data?}): Promise<{text, tools?}>` | Ask an employee, wait for the answer. `agent` names another employee (needs `subagent:<id>`). |
| `agents.stream(message, {agent?, data?}): AsyncGenerator<{text, done}>` | The same, streamed. |
| `janus.complete({messages, model?, temperature?, max_tokens?, system?}): Promise<string>` | A raw model call: no persona, no memory, no tools. |
| `janus.stream(same): AsyncGenerator<string>` | The same, streamed. |
| `nebo.fetch(pathOrUrl, init?)` — top level `NeboAppSDK.neboFetch` | Relative path goes to the app's own sidecar API; absolute `http(s)://` goes through Nebo's proxy (needs `network:<host>`). |
| `new nebo.WebSocket()` — top level `new NeboAppSDK.NeboWebSocket()` | Live socket to the app's employee at `/ws/app/<id>`; reconnects with backoff. `send(data)`, `close()`, `onopen/onmessage/onerror/onclose`. No arguments. |
| `surfaces.connect()`, `surfaces.on(type, handler)`, `surfaces.send(name, payload)`, `surfaces.state` | Typed events from the employee (`text_content`, `state_delta`, `surface_update`, ...). `on("*", h)` hears all. |
| `chat.mount(el, {placeholder?, theme?, height?, borderless?, contextId?, scope?})` | Nebo's chat with this employee, inside the page. |
| `chat.send(text)`, `chat.setContext(ctx \| null)`, `chat.onMessage(h)`, `chat.newThread()`, `chat.unmount()` | Drive the mounted chat. |
| `nebo.configure({appId?, baseUrl?})` | Only for a page served outside Nebo. It is a method on `nebo` alone; at the top level the same two settings are `NeboAppSDK.setAppId(id)` and `NeboAppSDK.setBaseUrl(url)`, and `getAppId()` / `getBaseUrl()` read them back. |

---

## A Minimal Page That Works

This is the whole page. It loads the SDK, asks who it is, and prints the answer.
Start every app from here and grow it.

```html
<!doctype html>
<html>
<head>
  <meta charset="utf-8">
  <title>Deal Board</title>
</head>
<body>
  <h1>Deal Board</h1>
  <pre id="who">loading...</pre>
  <script src="/sdk/nebo.global.js"></script>
  <script>
    NeboAppSDK.nebo.identity.get()
      .then(me => { document.getElementById('who').textContent = JSON.stringify(me, null, 2); })
      .catch(err => { document.getElementById('who').textContent = String(err); });
  </script>
</body>
</html>
```

Pass it as `ui: { "index.html": "<that page>" }`. If the page shows the
employee's id and name, the wiring is right; everything after that is ordinary
HTML and JavaScript against the table above.

---

## Iterate

Change an app the same way you made it — through the registry door. Never
hand-write the files: the tool is the one writer of an app's package, and it
checks the manifest, the permissions and every `ui` path before a byte lands.

```
agent(resource: "registry", action: "update", name: "deal-board",
      ui: { "index.html": "<the whole new page>" })
```

`update` takes the same `ui`, `ui_jsx`, `app` and `agent_md` the create took.
Each names what it replaces: a path in `ui` overwrites that one file and leaves
the rest of the folder alone; `app` changes only the fields it carries; passing
`agent_md` rewrites the persona. The page is read from disk on every request,
so reload the window and the change is there — nothing to restart, and a
manifest or persona change is picked up by the watcher within a few seconds.

Keep state in `storage`, not in the page: the window is closed and reopened,
and the store survives that.

---

## Two Things That Go Wrong

1. **The global is `NeboAppSDK`, not `nebo`.** A page written as
   `nebo.identity.get()` throws `ReferenceError: nebo is not defined`. Write
   `NeboAppSDK.nebo.identity.get()`, or take `const { nebo } = NeboAppSDK;`
   first.

2. **Deleting the folder does not delete the employee.** Removing
   `user/agents/<name>/` deactivates the employee (Nebo keeps the record so the
   folder can come back). To delete it, use the registry door:
   `agent(resource: "registry", action: "delete", name: "<name>")`, which removes
   the record, the live registry entry, and the folder.

---

## Vocabulary

Say employee and hire; say the owner. An app is "your <name> app", never a bot
or an assistant.
