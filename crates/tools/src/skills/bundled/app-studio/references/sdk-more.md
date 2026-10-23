# The rest of the SDK

The calls SKILL.md's table leaves out: the owner's files, sharing a file, streaming, cards, the embedded chat, and pages served outside Nebo.

| Call | Does |
|------|------|
| `files.url(path): string` | The address of one of the owner's files, for an `<img>`, `<video>`, `<audio>` or `fetch()`. `path` is relative to his NeboAI folder (`Media/Projects/reel/render.mp4`) and must be inside a folder the manifest grants with `filesystem:read:<folder>` (see below). |
| `share({name, content}): Promise<{artifact}>` | Hand the owner a file to share (a presentation, a report): it goes into his Work and Nebo opens its Share dialog on it, where he picks who can open the link, on the device the page is open on. Never build a share link yourself. |
| `agents.stream(message, {agent?, data?}): AsyncGenerator<{text, done}>` | The same, streamed. |
| `janus.stream(same): AsyncGenerator<string>` | The same, streamed. |
| `surfaces.connect()`, `surfaces.on(type, handler)`, `surfaces.send(name, payload)`, `surfaces.state` | Cards from the employee's `a2ui` tool. |
| `chat.mount(el, {placeholder?, theme?, height?, borderless?, contextId?, scope?})` | Nebo's chat with this employee, inside the page. `chat.send`, `chat.setContext`, `chat.onMessage`, `chat.newThread`, `chat.unmount` drive it. |
| `nebo.configure({appId?, baseUrl?})` | Only for a page served outside Nebo; at the top level `NeboAppSDK.setAppId(id)` / `NeboAppSDK.setBaseUrl(url)`, read back with `getAppId()` / `getBaseUrl()`. |
| `window.share_menu` entry `{"label": "Share a link", "say": "Share this as a link.", "share": true}` | A link from the header's Share: its `say` is sent into the chat; answer with ONE file, handed over in your reply (or by the page with `share`), and Nebo opens its Share dialog on it on the device it was picked on. |

## Showing the owner's files

A page shows the owner's own files (his photos, his renders, a project's
`project.json`) only through `files.url`, and only inside the folders its
manifest grants. Grant each folder in `app.permissions`, relative to his
NeboAI folder:

```
update_employee(name: "Reel", app: { permissions: ["storage:readwrite", "filesystem:read:Media/Projects"] })
```

```js
const { files } = NeboAppSDK;
video.src = files.url(`Media/Projects/${project}/versions/${name}-v3.mp4`);
const info = await fetch(files.url(`Media/Projects/${project}/project.json`)).then((r) => r.json());
```

- The address works on the desktop, in a browser and in the mobile app. Never
  write `/api/v1/files/...` or any other address by hand: it breaks in the
  mobile app.
- Read only. Videos stream with byte ranges, so they play and seek.
- A path with a `.` or `..` part throws. A path outside every granted folder
  answers 403, saying which permission is missing. A missing file answers 404,
  and so does anything hidden (any part starting with `.`, such as
  `.proxies` or `.env`) or in a coding agent's folder: those are never
  served. Put files the page shows in visible folders.
- A grant is a visible folder inside the NeboAI folder: no leading `/`, no
  `..`, no hidden part.
- An app you build here gets its grants as written. An app installed from the
  marketplace gets none until the owner allows it in plain words, and he can
  take it back in the app's settings.
- The page's own files (images and videos it ships) stay in `ui/`; `files.url`
  is for the owner's work.
