# Idea: native video recorder for Nebo (demo capture, GIFs, bug repros)

**Status:** researched + core unknown de-risked with a working spike. Not built.
**Date:** 2026-06-17
**Origin:** needing a Google OAuth-verification *demo video* for gws → "could Nebo record its own demos?" → evaluate `@alaarab/video-recorder-mcp` → decide build-vs-buy for a native Rust capability.

---

## The opportunity

A capability that lets Nebo **record polished product demos, GIFs, and bug repros of itself** — and, near-term, produce the Google OAuth verification demo video. Nebo already ships the two hard pieces: **ffmpeg** (in the plugins/marketplace system) and a **CDP browser stack** (`crates/browser` + bundled Obscura, driven via `chromiumoxide`).

## Reference implementation studied: `@alaarab/video-recorder-mcp`

(Not `@polaroid/...` — that npm name 404s; mcpmarket.com auto-generates listings. The real one is `github.com/alaarab/video-recorder-mcp`, v0.5.0, MIT.)

It's **one 3530-line ESM file**, deps `@modelcontextprotocol/sdk` + `playwright`, shelling out to system `ffmpeg`/`ffprobe`. ~20 MCP tools. No novel video tech — pure orchestration:

- **Job bundle** (`createVideoJob`): timestamped folder with `raw/final/clips/screens/diagnostics/analysis/assets` + `manifest.jsonl` event log + `job.json`. Maps cleanly onto our `NEBO_DATA_DIR` / `appdata/<type>/<slug>` contract.
- **Screen capture** (macOS only): `spawn("ffmpeg", ["-f","avfoundation","-capture_cursor",..,"-i","0:none","-c:v","libx264",..])`; stop by writing `"q\n"` to ffmpeg stdin.
- **Browser capture**: Playwright `recordVideo` (WebM→mp4 via ffmpeg) + injected CSS/JS **demo overlay** (synthetic cursor, click ripples, outline/spotlight annotations), pacing presets (`feature`/`bug`/`tutorial`/`gif`/`manual`), diagnostics (console/pageerror/network/trace/HAR).
- **Analysis**: `ffprobe -print_format json`, frame extraction, scene detect (`select='gt(scene,thr)',showinfo` → parse `pts_time`), contact sheet (`tile`), waveform (`showwavespic`).
- **Compose**: trim/crop/scale/GIF-palette, `xfade`/`acrossfade` filtergraphs. Title cards are rendered by launching **headless Chromium on an HTML string → screenshot → ffmpeg loops the PNG** into a clip.

## Key finding: Obscura cannot do the browser-capture half

Our bundled browser **Obscura** (`~/workspaces/nebo/obscura`, fork `localrivet/obscura`, branch `chromiumoxide-cdp-compat`) is a **from-scratch Rust scraping engine** — DOM + V8 + network + a hand-written CDP server (`obscura-cdp`). It runs real JS but has **no layout/paint engine**. Its own CDP handler returns, verbatim:

> *"Page.captureScreenshot is not supported by Obscura: no layout or paint engine. For visual snapshots, drive a real headless Chromium for the screenshot leg of your pipeline and use Obscura for the scraping leg."*

So: no screenshots → no `startScreencast` → **no video frames**. Obscura is the wrong half of the problem (it scrapes; it doesn't paint). Any visual capture must use a **real** Chrome/Brave or the OS screen.

## De-risked: real Chrome/Brave screencast via chromiumoxide WORKS

`chromiumoxide` (which we already depend on, 0.9.1) is just a CDP client — `cdp_bridge.rs` points it at Obscura, but it drives **real Brave/Chrome** equally well. A standalone spike launched real Brave, navigated to a live page, called `Page.startScreencast`, and pulled back a **pixel-perfect 800×600 PNG frame**. (Saved frame: rendered "Example Domain" page — real text/fonts/layout.)

This is the native browser-capture path: **chromiumoxide + real Brave/Chrome + `Page.startScreencast` → frames → ffmpeg**. No Obscura, no bundled Chromium, no node/Playwright.

### Spike (reproducible)

`/tmp/screencast-spike` — `Cargo.toml` uses `chromiumoxide = { version = "0.9", default-features = false }` (matches workspace; local CDP socket is plain `ws://`, no TLS feature needed). Core of `src/main.rs`:

```rust
let config = BrowserConfig::builder()
    .chrome_executable("/Applications/Brave Browser.app/Contents/MacOS/Brave Browser")
    .window_size(1280, 800)
    .build()?;
let (mut browser, mut handler) = Browser::launch(config).await?;
tokio::spawn(async move { while let Some(_ev) = handler.next().await {} });
let page = browser.new_page(url).await?;
page.wait_for_navigation().await?;
let mut frames = page.event_listener::<EventScreencastFrame>().await?;  // subscribe BEFORE start
page.execute(StartScreencastParams::builder()
    .format(StartScreencastFormat::Png).every_nth_frame(1).build()).await?;
// each frame: AsRef::<str>::as_ref(&frame.data) is base64 → decode → bytes;
// MUST ack: page.execute(ScreencastFrameAckParams::new(frame.session_id)).await
```

Gotchas hit while building it:
- chromiumoxide 0.9.1 dropped the `tokio-runtime` feature (runtime-agnostic now). Enabling `rustls`/defaults drags in `chromiumoxide_fetcher`, which fails to compile without a `zip0`/`zip8` feature → just use `default-features = false`, no features.
- `frame.data` is `chromiumoxide::Binary(String)` = **raw base64** (both `AsRef` impls give the undecoded string); decode it yourself.
- **You must `ScreencastFrameAck` every frame** or Chrome stops sending.

## Proposed design: `crates/recorder`

Two capture engines feeding one job-bundle + compose layer (all ffmpeg). Surface as actions on the existing domain tools — **not** a separate MCP server:

- **`os`/`desktop` tool → screen capture.** ffmpeg grabbing the real display. Cross-platform input matrix is the only real net-new work:
  - macOS → `avfoundation` (`-i "<screen-idx>:none"`)
  - Windows → `gdigrab` / `ddagrab`
  - Linux → `x11grab` / `kmsgrab`
  - Captures whatever's actually on screen (real Brave, the Nebo app). **This is the right tool for the Google verification video** (real account sessions, real chrome).
- **`web` tool → browser-demo capture.** chromiumoxide → real Chrome/Brave (`.chrome_executable`) + `startScreencast` (proven) + inject the overlay JS via `Page.addScriptToEvaluateOnNewDocument`. Polished synthetic-cursor demos of web flows.
- **Shared**: job bundle (`appdata/recorder/<job>/…` + `manifest.jsonl`) and the analyze/clip/compose ffmpeg orchestration (direct port of the reference's arg-vectors).

### Open questions before/while building
- **Screencast is change-driven**: Chrome emits a frame only when pixels change. Static page = 1 frame (observed). Each frame has `metadata.timestamp` → feed ffmpeg as VFR. For guaranteed steady framerate or full-OS capture, use the desktop path. **Spike #2 worth doing: frame cadence under real interaction (navigate→click→type).**
- **Resolution**: screencast defaulted to 800×600 despite `window_size(1280,800)`. Pass `max_width`/`max_height` + a sensible `deviceScaleFactor` explicitly.
- **Title cards**: reference renders them via headless Chromium+HTML. We can do the same (real Chrome path) or simplify with ffmpeg `drawtext`/an SVG→png to avoid a browser dependency for cards.
- **Profile lock** (for real-session captures like the Google video): Chrome/Brave allow one process per `--user-data-dir`. Use a dedicated debug profile (sign into the demo account once) rather than hijacking the user's live browser.

## Build vs. buy

- **Fastest (interim):** consume `@alaarab/video-recorder-mcp` as-is via Nebo's **MCP stdio client / CONN- connector** — zero rewrite. Cost: runtime deps on `node` + the npm package + Playwright Chromium.
- **Native (recommended, post-launch):** `crates/recorder` as above. No node, cross-platform, reuses ffmpeg + chromiumoxide. The desktop-capture half is small and self-contained; the browser half is now de-risked.

## Priority note

This is **not** on the critical path to launch. The launch gate is the gws Google OAuth **verification** (restricted scopes `gmail.modify`, `drive` → brand review + CASA Tier 2 security assessment, ~4–8 wks). The demo video that verification needs can be recorded today with QuickTime or the npm tool against real Brave. Treat the native recorder as a real but separate feature.
