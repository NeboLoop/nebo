# The rest of the SDK

The calls SKILL.md's table leaves out: sharing a file, streaming, cards, the embedded chat, and pages served outside Nebo.

| Call | Does |
|------|------|
| `share({name, content}): Promise<{artifact}>` | Hand the owner a file to share (a presentation, a report): it goes into his Work and Nebo opens its Share dialog on it, where he picks who can open the link. Never build a share link yourself. |
| `agents.stream(message, {agent?, data?}): AsyncGenerator<{text, done}>` | The same, streamed. |
| `janus.stream(same): AsyncGenerator<string>` | The same, streamed. |
| `surfaces.connect()`, `surfaces.on(type, handler)`, `surfaces.send(name, payload)`, `surfaces.state` | Cards from the employee's `a2ui` tool. |
| `chat.mount(el, {placeholder?, theme?, height?, borderless?, contextId?, scope?})` | Nebo's chat with this employee, inside the page. `chat.send`, `chat.setContext`, `chat.onMessage`, `chat.newThread`, `chat.unmount` drive it. |
| `nebo.configure({appId?, baseUrl?})` | Only for a page served outside Nebo; at the top level `NeboAppSDK.setAppId(id)` / `NeboAppSDK.setBaseUrl(url)`, read back with `getAppId()` / `getBaseUrl()`. |
