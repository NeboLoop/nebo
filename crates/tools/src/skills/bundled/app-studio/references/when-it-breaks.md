# When It Breaks

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

## An app built with a bundler, now blank

A page built with Vite or Bun can reference a code-split chunk that was never
written (`main-<hash>.js` imports `./SomeComponent`, which is missing). Don't
rebuild with the bundler; move it to Lane A:

1. Find the source (`src/`, or the app folder). Read it with `code(action: "outline")`.
2. Split it into the Step 3A layout: `app.jsx` shell, `store.js`, one file per
   screen, each under about 12 KB.
3. Send `index.html` (the relative SDK tag, then `./app.js`), `app.jsx` and
   `store.js` in one `update_employee` call; `app_reload`; the page loads.
4. Send the screens, a few per call, reloading after each.
5. Delete the old bundle files from `ui/` (`main-*.js`, `assets/index-*.js`,
   `dist/`) once the new page works.

