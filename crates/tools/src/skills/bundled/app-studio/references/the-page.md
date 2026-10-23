# What a page can reach

An app's page is confined to its own app, wherever it opens, and never
holds the owner's sign-in. Over HTTP (a browser, the mobile app, the
screenshot and Verified checks) it runs sandboxed, in an origin of its own:
`/k/<pass>/` in its address is its pass, Nebo adds it when the page opens,
everything it loads from its own address carries it, and it opens this
app's routes alone. The desktop window is an origin of its own
(`neboapp://<id>`) that reaches only this app's routes. The SDK carries
the right address on every call.

| The page needs | Use |
|---|---|
| Its records | `storage` (the employee's `app_data` reads the same store) |
| `localStorage` | Over HTTP Nebo keeps it in the app's store under the key `localStorage`; in the desktop window it is that window's own, not shared with the mobile app. `sessionStorage` lasts as long as the page. Anything that must be the same everywhere, and every record the employee reads, goes in `storage` |
| The owner's files (photos, renders, a `project.json`) | `files.url(path)` inside folders granted with `filesystem:read:<folder>` (sdk-more.md). Never copy them into `ui/` or storage |
| A plugin's work at once (a thumbnail, a waveform, a trim) | `plugins.run(slug, command, args)` with `plugin:<slug>` (sdk-more.md) |
| Work that needs judgment | `agents.invoke` to its employee: the answer is its words and which tools ran, never what a tool gave back |
| A chat with its employee | `chat.mount` (sdk-more.md): drawn in the page, talks only to this app's own employee. Its words are the app's, not the owner's: codes, slash commands, file paths and @mentions are just words |
| Another website | `nebo.fetch('https://...')` with `network:<host>`: public sites only, never this computer or its network, whatever a granted name points to; every redirect is held to the granted hosts. Text only: a body goes as text (no `FormData` or `Blob`) and a picture or other binary answer arrives broken; fetch images with an `<img>` instead |
| Another employee | `agents.invoke(message, {agent})` with `subagent:<id>` |
| A link out | `target="_blank"` or `window.open`: a new window |

Never: `/api/v1/files`, another app, or any other Nebo API. The page's
socket carries its data changes, its cards and its own chat's words, nothing
else of the owner's.

## The owner's OK

An app built here holds what its manifest asks for. One installed from the
marketplace or shared through a loop gets `filesystem:`, `network:`,
`subagent:` and `plugin:` only once the owner allows each, and he can take it
back in the app's settings. A refused call answers 403 naming the
permission (`nebo.fetch` returns it as its `Response`, status and text):
the page says plainly what it can't do yet. Never ask the owner
to get around it.
