# Sidecars

A sidecar is a native program shipped with the app (one binary per platform)
that the page reaches with `nebo.fetch('/path')`. It is a developer job, not
something built in a chat: it needs a compiler, a build per platform and a
publisher account. Many needs are covered without one; check first.

## What covers it without a sidecar

| The app needs | Use |
|---|---|
| To keep records | `storage` on the page, `app_data` for the employee: one store |
| To call an outside service | `nebo.fetch('https://...')` through Nebo's proxy, with `network:<host>` in the manifest |
| Work on a schedule, or while the page is closed | The employee's own workflows (`automations`) |
| AI: text, a judgment, a picture | `janus.complete`, `decide`, the employee's `generate_media` |
| A file made for the owner | The employee makes it and hands it over with `share_file` |
| Another employee's help | `agents.invoke` (needs `subagent:<id>`) |

## When a sidecar is right

- A real database or heavy data work the page can't do (a large SQLite, an index).
- A long-lived connection: a device, a socket to another system, a watcher.
- A native library or a program on the computer the page can't reach.
- A secret the page must never see, used on the server side only.

Say so plainly to the owner, and that it means a developer building the binary.
The full contract (the gRPC service, environment, tools declared in
`agent.json`, health checks) is in the publishers guide, Apps → Sidecar Binary.

## Which language

Go, with the official `nebo-sdk-go` and the `app-boilerplate` template: one
static binary per platform (macOS, Linux and Windows, arm64 and x86_64) built
from one machine (`make build-all`), plain code that builds in seconds. Rust
(`nebo-sdk-rust`) when the work needs its speed or a native library; C
(`nebo-sdk-c`) only to wrap an existing C library.

