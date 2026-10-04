//! `app_record`: an app's page played and saved as numbered PNG frames, a
//! motion post or a trailer that Nebo Media's `video encode` turns into a
//! video.
//!
//! The page opens the way `app_screenshot` opens it (`app_publish::AppPage`:
//! the bot's own headless browser, the app as the bot serves it, watched
//! for errors from its first script). The recording is not a screen
//! capture: the page's clock stands still and is moved one frame at a time
//! (`app_record/clock.js`, installed before the page's own scripts: timers,
//! animation frames, `performance.now`, `Date`, CSS, Web and SVG animations
//! and videos all follow it). Each frame is drawn whole at its own moment,
//! however slow the machine, so a 10-second post at 30 fps is exactly 300
//! frames, evenly spaced. Stepping the page's own clock was chosen over the
//! browser's virtual time (`Emulation.setVirtualTimePolicy`): paused virtual
//! time stalls the frame a screenshot waits for in new headless Chrome, and
//! the clock here works the same on any Chrome or Chromium the bot finds.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::app_publish::{
    AppPage, failed_load, headless, load_console, page_path, permitted_app, safe_name, viewport, warning_lines,
};
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

pub const APP_RECORD: &str = "app_record";

/// The page's clock, moved by the recorder.
const CLOCK: &str = include_str!("app_record/clock.js");
/// The longest recording, the highest frame rate, and the most frames
/// one recording makes (a minute at 30 fps).
const MAX_SECONDS: f64 = 60.0;
const MAX_FPS: u64 = 60;
const MAX_FRAMES: u64 = 1800;

/// What a request says when it wants an app recorded (`DynTool::triggers`):
/// a request that says one has the tool loaded on its first step. Live
/// 2026-10-03: "Record 3 seconds of Kart Racer's page and make it an mp4"
/// left it deferred, and the employee said it had no way to record a page.
const TRIGGERS: &[&str] = &[
    "record the app",
    "record my app",
    "record the page",
    "app recording",
    "motion post",
    "trailer",
    "screen record",
    "screen recording",
];

/// What `video encode` reads beside the frames (Nebo Media checks it
/// against the folder).
#[derive(Debug, serde::Serialize)]
struct Manifest {
    width: u32,
    height: u32,
    fps: u64,
    frames: u64,
}

pub struct AppRecordTool {
    store: Arc<db::Store>,
    browser: Option<Arc<browser::Manager>>,
    triggers: Vec<String>,
}

impl AppRecordTool {
    pub fn new(store: Arc<db::Store>, browser: Option<Arc<browser::Manager>>) -> Self {
        Self { store, browser, triggers: TRIGGERS.iter().map(|t| t.to_string()).collect() }
    }

    async fn record(&self, ctx: &ToolContext, input: &Value) -> ToolResult {
        let app = match permitted_app(&self.store, ctx, input["app"].as_str()) {
            Ok(a) => a,
            Err(e) => return ToolResult::error(e),
        };
        let page = match page_path(&app, input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(e),
        };
        let fps = input["fps"].as_u64().unwrap_or(30);
        if !(1..=MAX_FPS).contains(&fps) {
            return ToolResult::error(format!("fps is 1 to {MAX_FPS}."));
        }
        let seconds = input["seconds"].as_f64().unwrap_or(0.0);
        if !(seconds > 0.0 && seconds <= MAX_SECONDS) {
            return ToolResult::error(format!("seconds is more than 0 and at most {MAX_SECONDS}."));
        }
        let count = (seconds * fps as f64).round() as u64;
        if count == 0 {
            return ToolResult::error("That is less than one frame: record longer or at a higher fps.");
        }
        if count > MAX_FRAMES {
            return ToolResult::error(format!(
                "That is {count} frames; a recording makes at most {MAX_FRAMES} (a minute at 30 fps)."
            ));
        }
        let executor = match headless(self.browser.as_ref(), "record the app") {
            Ok(e) => e,
            Err(e) => return ToolResult::error(e),
        };
        let size = viewport(&app, input, (1080, 1080));
        let wait = Duration::from_millis(input["wait_ms"].as_u64().unwrap_or(1000).min(10_000));

        let files = match config::data_dir() {
            Ok(d) => d.join("files"),
            Err(e) => return ToolResult::error(format!("cannot find the workspace: {e}")),
        };
        let folder = files
            .join("app-recordings")
            .join(safe_name(&app.name))
            .join(format!(
                "{}-{}x{}-{fps}fps",
                chrono::Utc::now().format("%Y%m%d-%H%M%S%3f"),
                size.0,
                size.1
            ));
        if let Err(e) = std::fs::create_dir_all(&folder) {
            return ToolResult::error(format!("could not make the recording's folder: {e}"));
        }

        let console_mark = crate::app_console::recent(&app.id, None, 1).last().map(|e| e.seq);
        // The pass lasts the load and a second of work per frame.
        let ttl = wait + Duration::from_secs(120 + count);
        let recorded = match AppPage::open(&executor, &app, &page, size, &[CLOCK], ttl).await {
            Ok(view) => {
                let r = play(&view, &app, &page, wait, fps, count, &folder).await;
                view.close().await;
                r
            }
            Err(e) => Err(e),
        };
        let console = load_console(&crate::app_console::recent(&app.id, console_mark, 200));
        match recorded {
            Ok((width, height, warnings)) => ToolResult::ok(format!(
                "Recorded {count} frames of {} ({page}) at {width}x{height}, {fps} fps, {seconds} s, into {}. \
                 manifest.json beside them gives the size, frame rate and frame count. Make the video with Nebo \
                 Media's `video encode` (frames: that folder; the `video` skill), then give it to the owner with \
                 share_file.{}{console}",
                app.name,
                folder.display(),
                warning_lines(&warnings)
            )),
            Err(e) => {
                // Nothing half-made is left behind.
                let _ = std::fs::remove_dir_all(&folder);
                ToolResult::error(format!("{e}{console}"))
            }
        }
    }
}

/// Let the page load, then draw `count` frames, each at its own moment of
/// the page's clock, into `folder` as `frame_00001.png`…, and write the
/// manifest. The size the frames were drawn at comes back, with the
/// page's warnings (a font that failed). A page that fails, on load or
/// while it plays, is an error; so is a picture that did not load, checked
/// after the load and again at the end.
async fn play(
    view: &AppPage<'_>,
    app: &db::models::Agent,
    page: &str,
    wait: Duration,
    fps: u64,
    count: u64,
    folder: &Path,
) -> Result<(u32, u32, Vec<String>), String> {
    tokio::time::sleep(wait).await;
    // Its fonts in, so the first frame is drawn with them.
    view.eval("document.fonts ? document.fonts.ready.then(() => 'ok') : 'ok'").await?;
    let mut warnings: Vec<String> = Vec::new();
    let mut check = async |images: bool| {
        let checked = view.check(images).await?;
        if !checked.failed.is_empty() {
            return Err(failed_load(app, page, &checked.failed));
        }
        for w in checked.warnings {
            if !warnings.contains(&w) {
                warnings.push(w);
            }
        }
        Ok(())
    };
    check(true).await?;
    let mut size = None;
    for i in 0..count {
        let at = i as f64 * 1000.0 / fps as f64;
        view.eval(&format!("window.__neboClock.to({at})")).await?;
        let shot = view.call("screenshot", &json!({ "format": "png" })).await?;
        let bytes = shot["data"]
            .as_str()
            .and_then(|d| base64::Engine::decode(&base64::engine::general_purpose::STANDARD, d).ok())
            .ok_or("The browser returned no picture.")?;
        if size.is_none() {
            size = Some(png_size(&bytes).ok_or("The browser's picture is not a PNG.")?);
        }
        std::fs::write(frame_path(folder, i + 1), &bytes).map_err(|e| format!("could not save frame {}: {e}", i + 1))?;
        // A page that breaks while it plays stops the recording within a second.
        if (i + 1) % fps == 0 {
            check(false).await?;
        }
    }
    check(true).await?;
    let (width, height) = size.ok_or("The browser returned no picture.")?;
    let manifest = Manifest { width, height, fps, frames: count };
    std::fs::write(
        folder.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
    )
    .map_err(|e| format!("could not save manifest.json: {e}"))?;
    Ok((width, height, warnings))
}

/// Frame `n` (from 1), as `video encode` finds a sequence.
fn frame_path(folder: &Path, n: u64) -> PathBuf {
    folder.join(format!("frame_{n:05}.png"))
}

/// A PNG's width and height, from its header.
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((w, h))
}

impl DynTool for AppRecordTool {
    fn name(&self) -> &str {
        APP_RECORD
    }

    fn description(&self) -> String {
        format!(
            "Records an app's page as numbered PNG frames for a motion post or a trailer, in this bot's own headless browser.\n\
             - The page's clock is stepped one frame at a time (timers, animation frames, CSS and Web animations, videos), so every frame is drawn whole and evenly spaced. 10 seconds at 30 fps is exactly 300 frames.\n\
             - `app`: the app's name; leave it out when you are the app. `path`: the page inside the app (default index.html, e.g. \"render.html?key=launch\").\n\
             - `width`/`height`: the frame size (default 1080x1080). `seconds` (at most {MAX_SECONDS}) and `fps` (default 30, at most {MAX_FPS}); at most {MAX_FRAMES} frames.\n\
             - A page that fails to load or throws while it plays, or shows a picture that did not load, is an error; nothing is kept.\n\
             - The frames and a manifest.json go in a folder in the workspace under app-recordings/; Nebo Media's `video encode` makes the video from that folder."
        )
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "app": { "type": "string", "description": "The app's name or id. Leave out when you are the app." },
                "path": { "type": "string", "description": "The page inside the app, e.g. render.html?key=launch." },
                "width": { "type": "integer", "description": "Frame width in pixels." },
                "height": { "type": "integer", "description": "Frame height in pixels." },
                "seconds": { "type": "number", "description": "How long the recording plays." },
                "fps": { "type": "integer", "description": "Frames per second (default 30)." },
                "wait_ms": { "type": "integer", "description": "How long the page loads before the first frame (default 1000, at most 10000)." }
            },
            "required": ["seconds"]
        })
    }

    fn search_hint(&self) -> &str {
        "record an app as video frames"
    }

    fn triggers(&self) -> &[String] {
        &self.triggers
    }

    fn read_only(&self, _input: &Value) -> bool {
        true
    }

    fn concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    fn activity(&self, _input: &Value) -> String {
        "recording the app".to_string()
    }

    fn outcome(&self, _input: &Value) -> String {
        "Recorded the app".to_string()
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move { self.record(ctx, &input).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_numbered_for_video_encode() {
        assert!(frame_path(Path::new("/x"), 1).ends_with("frame_00001.png"));
        assert!(frame_path(Path::new("/x"), 300).ends_with("frame_00300.png"));
    }

    #[test]
    fn a_png_header_gives_its_size() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend(1080u32.to_be_bytes());
        png.extend(1920u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((1080, 1920)));
        assert_eq!(png_size(b"GIF89a"), None);
    }

    #[test]
    fn the_manifest_is_what_video_encode_reads() {
        let m = Manifest { width: 1080, height: 1080, fps: 30, frames: 300 };
        assert_eq!(
            serde_json::to_value(&m).unwrap(),
            json!({"width": 1080, "height": 1080, "fps": 30, "frames": 300})
        );
    }
}
