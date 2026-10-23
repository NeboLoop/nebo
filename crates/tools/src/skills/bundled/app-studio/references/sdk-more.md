# The rest of the SDK

The calls SKILL.md's table leaves out: sharing a file, streaming, cards, and the embedded chat.

| Call | Does |
|------|------|
| `share({name, content}): Promise<{artifact}>` | Hand the owner a file to share (a presentation, a report): it goes into his Work and Nebo opens its Share dialog on it, where he picks who can open the link, on the device the page is open on. Never build a share link yourself. |
| `agents.stream(message, {agent?, data?}): AsyncGenerator<{text, done}>` | The same, streamed. |
| `janus.stream(same): AsyncGenerator<string>` | The same, streamed. |
| `surfaces.connect()`, `surfaces.on(type, handler)`, `surfaces.send(name, payload)`, `surfaces.state` | Cards from the employee's `a2ui` tool. |
| `chat.mount(el, {placeholder?, theme?, height?, borderless?, contextId?, scope?})` | Nebo's chat with this employee, inside the page. `chat.send`, `chat.setContext`, `chat.onMessage`, `chat.newThread`, `chat.unmount` drive it. |
| `nebo.configure({appId?, baseUrl?})`, `NeboAppSDK.setAppId` / `setBaseUrl` | Never call them: Nebo sets the app and its address on every page it serves (read back with `getAppId()` / `getBaseUrl()`). A page served from anywhere else gets no answer from Nebo. |
| `window.share_menu` entry `{"label": "Share a link", "say": "Share this as a link.", "share": true}` | A link from the header's Share: its `say` is sent into the chat; answer with ONE file, handed over in your reply (or by the page with `share`), and Nebo opens its Share dialog on it on the device it was picked on. |
