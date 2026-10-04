# Your App's Data, in full

The detail SKILL.md's "Your App's Data" leaves out: every `app_data` action, paths and replace, and one piece of work per chat.

The page's `storage` and the employee's `app_data` tool are one store: same
keys, same values. What the owner tells one, the other sees.

```
app_data(action: "set",   key: "contacts", value: [{ "name": "John Smith", "phone": "+1 555 0100" }])
app_data(action: "get",   key: "contacts")
app_data(action: "set",   key: "site", path: "screens.2.html", value: "<main>...</main>")
app_data(action: "replace", key: "page", find: "<h1>Old headline</h1>", with: "<h1>New headline</h1>")
app_data(action: "query", where: { "name": "john smith" })
app_data(action: "query", text: "smith", prefix: "contact:", limit: 5)
app_data(action: "list",  prefix: "contact:")
app_data(action: "delete", key: "draft")
```

- Only the app's own employee has the tool, for its own app. A coworker asks it.
- After a `set` or `delete` every open view hears it: `storage.onChange(() => load())`.
- One key holding a list (`contacts`), or one key per record (`contact:<id>`).
  Write the keys into the employee's instructions.
- `value` is JSON itself (an object, a list), never JSON inside a string.
  JSON-looking text that does not parse is refused: "Nothing was saved".
- `path` (dotted, list items by index) sets or gets one spot. Replies cap
  near 8k tokens: save a small skeleton, then fill one piece per call.
- `replace` changes one exact piece of text (a heading in a page).
  `find` must match one place, or nothing changes.

## One piece of work per chat

An app that holds one thing per conversation (a design, a draft, a plan)
keeps it under a `chat:` key. `chat:design` written from the owner's chat
`<id>` is stored as `chat:<id>:design`; the page opened from that chat gets
`?thread=<id>` (desktop and phone) and reads that key. The employee never
needs the chat id:

```js
const thread = new URLSearchParams(location.search).get('thread');
const key = thread ? `chat:${thread}:design` : null;   // null: opened from home, show a gallery
```

- The employee's instructions say: get `chat:design` first every turn, change
  it with `set` or `replace`, and never end a turn with the work only in a
  file.
- Outside the owner's chats (a schedule, a caller) a `chat:` key is refused.
- Opened from home there is no thread: list `storage.keys()` matching
  `/^chat:[^:]+:design$/` for a gallery.
- Create such an employee with sealed conversations, so one chat's work never
  leaks into another: `agent_json: { "memory": { "mode": "confidential" } }`
  on `create_employee`. `"single"` (one conversation) is the default;
  `"separate"` is many chats sharing one memory.
- If the owner directs it by talking while looking at the page, add
  `window: { voice: true }` for the dictate and voice buttons in the phone bar.
