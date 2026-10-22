# Sidecars

A sidecar is a native program in the app's folder that Nebo runs beside the
page. The page reaches it with `nebo.fetch('<path>')`; the employee reaches it
through tools declared in `agent.json`. Most needs are met without one.

## What covers it without a sidecar

| The app needs | Use |
|---|---|
| To keep records | `storage` on the page, `app_data` for the employee: one store |
| To call an outside service | `nebo.fetch('https://...')` through Nebo's proxy, with `network:<host>` in the manifest |
| To react when files land in a folder | A workflow with a `folder` trigger (below); its run writes what the page shows with `app_data`, and the page hears it through `storage.onChange` |
| Work on a schedule, or while the page is closed | The employee's own workflows (`automations`) |
| A plugin's work (Nebo Media, Gmail) | `plugins.run` on the page with `plugin:<slug>` for work at once; the employee with its tools (`agents.invoke`) for work that needs judgment |
| AI: text, a judgment, a picture | `janus.complete`, `decide`, the employee's `generate_media` |
| A file made for the owner | The employee hands it over with `share_file`, the page with `share` |
| Another employee's help | `agents.invoke` (needs `subagent:<id>`) |

A folder trigger, in the `agent_json` of `create_employee` (each change batch
starts a run with the files in `_watch_payload.files`):

```json
{ "workflows": { "new-renders": {
    "trigger": { "type": "folder", "path": "/Users/<owner>/NeboAI/Media/Projects", "extensions": ["mp4", "json"] },
    "activities": [{ "id": "index", "intent": "Read the changed files and update the projects key with app_data." }] } } }
```

## When a sidecar is right

- An index or database too large or too busy for the store.
- A long-lived connection: a device, a socket to another system.
- A native library or a program the employee's tools can't drive.
- A secret used on the server side only, which the page must never see.

## Tell the owner these limits first

- **It runs on the bot's own computer** (macOS, Linux or Windows): one
  native program per platform, built for each one the app runs on.
- **Not published from a conversation.** `app_listing` and `app_submit`
  refuse an app that runs a program; that needs the full publishing tools.
- **It needs a compiler on the bot** (for Rust: `cargo`). Never install one
  (hard rule 4).

## What Nebo does with it

- **Finds it** in the app folder: the one file in `bin/`, else the
  executable in `sidecar/target/release/`. A native binary only: no script,
  no symlink, executable, under 500 MB.
- **Runs it** at boot and on the first request; restarts it when it crashes
  and when the file changes (a rebuild is a restart).
- **Gives it** `NEBO_APP_ID`, `NEBO_APP_NAME`, `NEBO_APP_VERSION`,
  `NEBO_APP_DIR` (the app folder), `NEBO_DATA_DIR` (the one place to
  write, kept across updates; also its working directory), `NEBO_API_URL`,
  `NEBO_APP_TOKEN` (for calling Nebo), `NEBO_SIDECAR_TOKEN` (what Nebo's
  calls to it carry; the SDK checks it and keeps it from your code), plus
  `PATH`, `HOME`, `TMPDIR`, `LANG`, `LC_ALL`, `TZ` and, on Windows,
  `SystemRoot`, `TEMP`, `TMP` and `USERPROFILE`. Nothing else.
- **Logs** its stdout and stderr to `$NEBO_DATA_DIR/sidecar.log`: read it
  first when it fails.
- **Holds its stdin open.** End of input means Nebo is gone: the SDK exits.
- **Talks to it** over gRPC on loopback: the SDK listens on `127.0.0.1` on a
  port the system picks and tells Nebo where; Nebo calls the `UIService` in
  `proto/apps/v0/ui.proto`, one `HandleRequest` per request, and proves it is
  the program it started (the SDK answers a health check). Use the SDK: a
  program that skips it is stopped. Nothing of the owner's (his sign-in, the
  page's pass) ever reaches it.

## How `nebo.fetch` reaches it

```
page:     nebo.fetch('notes?day=1', { method: 'POST', body: JSON.stringify(note) })
Nebo:     /api/v1/apps/<id>/api/notes?day=1        (the SDK adds the prefix)
sidecar:  HandleRequest { method: "POST", path: "notes", query: "day=1", headers, body }
page:     the sidecar's status, headers and body
```

- A path starting with `_` is refused; a body is at most 10 MB.
- While it starts or restarts the page gets 503 with `Retry-After` and the
  reason in `error`: show it and try again.

Tools for the employee go in `agent.json`. Each becomes the tool
`app__<app>__<name>`, sent through the same `HandleRequest` (GET input as
the query, other input as the JSON body, `{param}` filled from the input):

```json
{ "tools": [{ "name": "add_note", "description": "Add a note.", "method": "POST", "path": "/notes",
              "input_schema": { "type": "object", "properties": { "text": { "type": "string" } } } }] }
```

A tool's path keeps its leading `/` and a page's does not: trim it in the
handler. The sidecar reaches its own app's routes, and only those, at
`$NEBO_API_URL/api/v1/apps/$NEBO_APP_ID/...` with
`Authorization: Bearer $NEBO_APP_TOKEN`: `storage/<key>`, `agents/invoke`
(ask its employee, which runs plugins), `janus/complete`.

## A minimal Rust sidecar

`<app folder>/sidecar/Cargo.toml`:

```toml
[package]
name = "notes-sidecar"
version = "0.1.0"
edition = "2021"

[dependencies]
nebo-sdk = { git = "https://github.com/NeboLoop/nebo-sdk-rust", rev = "075f6cd2ba0521d05e7e3f010a99c6c7480bd85f" }
async-trait = "0.1"
tokio = { version = "1", features = ["full"] }
serde_json = "1"
```

`<app folder>/sidecar/src/main.rs`:

```rust
use async_trait::async_trait;
use nebo_sdk::ui::{HttpRequest, HttpResponse, UiHandler};
use nebo_sdk::{NeboApp, NeboError};
use std::collections::HashMap;

struct Api;

#[async_trait]
impl UiHandler for Api {
    async fn handle_request(&self, req: HttpRequest) -> Result<HttpResponse, NeboError> {
        let (status, body) = match (req.method.as_str(), req.path.trim_start_matches('/')) {
            ("GET", "hello") => (200, serde_json::json!({ "hello": std::env::var("NEBO_APP_NAME").unwrap_or_default() })),
            _ => (404, serde_json::json!({ "error": "Not found." })),
        };
        Ok(HttpResponse {
            status_code: status,
            headers: HashMap::from([("content-type".into(), "application/json".into())]),
            body: body.to_string().into_bytes(),
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Listens on loopback, answers Nebo's health check, and returns when
    // Nebo closes its stdin.
    NeboApp::new()?.register_ui(Api).run().await?;
    Ok(())
}
```

Build it in `<app folder>/sidecar` with `cargo build --release`; Nebo runs
`sidecar/target/release/notes-sidecar`. The data folder is `NEBO_DATA_DIR`
(the SDK's `env().data_dir`). The page calls `NeboAppSDK.neboFetch('hello')`; prove it with
`app_screenshot`, and `sidecar.log` when it fails.
