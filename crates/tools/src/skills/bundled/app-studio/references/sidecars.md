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
| A plugin's work (Nebo Media, Gmail) | The employee runs the plugin with its tools; the page asks it with `agents.invoke` |
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

- **macOS and Linux only.** On Windows, Nebo can't reach a sidecar: every
  `nebo.fetch` to it answers 503.
- **Not on macOS for an app made on this bot, today.** Its socket is
  `<app folder>/<app id>.sock`, and under
  `~/Library/Application Support/Nebo/user/agents/` that path is longer than
  macOS allows a socket (103 bytes), so the program can't listen. Say so;
  never work around it.
- **Not published from a conversation.** `app_listing` and `app_submit`
  refuse an app that runs a program; that needs the full publishing tools.
- **It needs a compiler on the bot** (for Rust: `cargo`, and `protoc` for the
  SDK's build). Never install one (hard rule 4).

## What Nebo does with it

- **Finds it** in the app folder: the one file in `bin/`, else the
  executable in `sidecar/target/release/`. A native binary only: no script,
  no symlink, executable, under 500 MB.
- **Runs it** at boot and on the first request; restarts it when it crashes
  and when the file changes (a rebuild is a restart).
- **Gives it** `NEBO_APP_ID`, `NEBO_APP_NAME`, `NEBO_APP_VERSION`,
  `NEBO_APP_DIR` (the app folder), `NEBO_DATA_DIR` (the one place to
  write, kept across updates; also its working directory), `NEBO_APP_SOCK`, `NEBO_API_URL`,
  `NEBO_APP_TOKEN`, plus `PATH`, `HOME`, `TMPDIR`, `LANG`, `LC_ALL` and `TZ`.
  Nothing else.
- **Logs** its stdout and stderr to `$NEBO_DATA_DIR/sidecar.log`: read it
  first when it fails.
- **Holds its stdin open.** End of input means Nebo is gone: exit.
- **Talks to it** over gRPC on `NEBO_APP_SOCK`: the `UIService` in Nebo's
  `proto/apps/v0/ui.proto`, one `HandleRequest` per request.

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
nebo-sdk = { git = "https://github.com/NeboLoop/nebo-sdk-rust", rev = "022c85bd21813ee2bdc280f701ac5e983497c001" }
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
use std::io::Read;

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
    // Nebo holds stdin open; end of input means Nebo is gone.
    std::thread::spawn(|| {
        let mut buf = [0u8; 64];
        while matches!(std::io::stdin().read(&mut buf), Ok(n) if n > 0) {}
        std::process::exit(0);
    });
    NeboApp::new()?.register_ui(Api).run().await?;
    Ok(())
}
```

Build it in `<app folder>/sidecar` with `cargo build --release`; Nebo runs
`sidecar/target/release/notes-sidecar`. Read the data folder from
`NEBO_DATA_DIR` yourself: the SDK's `env().data_dir` reads an old name and is
empty. The page calls `NeboAppSDK.neboFetch('hello')`; prove it with
`app_screenshot`, and `sidecar.log` when it fails.
