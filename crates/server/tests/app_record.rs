//! `app_record` and `app_screenshot` against the real app server, in the
//! bot's own headless browser: a recording is the page played on a stepped
//! clock (exactly fps × seconds frames, each its own moment, the same
//! frames every time), a page that fails is an error from both tools, and a
//! page other than index.html (`render.html`) carries the SDK's tags.
//! Skipped when this machine has no Chrome or Chromium.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::TestServer;
use serde_json::{Value, json};
use tools::registry::DynTool;

/// A box slid by a CSS animation, a bar drawn on a canvas from the
/// animation frame's time, and the time written out. Each frame's work is
/// made slow on purpose: a screen capture would drop or repeat frames.
const ANIMATED: &str = r#"<!doctype html><html><head><style>
body{margin:0;background:#111}
#box{position:absolute;top:10px;left:0;width:40px;height:40px;background:#e33;animation:slide 1s linear infinite}
@keyframes slide{from{transform:translateX(0)}to{transform:translateX(280px)}}
#t{position:absolute;bottom:8px;left:8px;color:#fff;font:16px monospace}
canvas{position:absolute;top:70px;left:0}
</style></head><body><div id="box"></div><canvas id="c" width="320" height="60"></canvas><div id="t"></div>
<script>
var c = document.getElementById('c').getContext('2d');
function draw(t) {
  var x = 0; for (var i = 0; i < 2e7; i++) x += i;
  c.fillStyle = '#000'; c.fillRect(0, 0, 320, 60);
  c.fillStyle = '#3e3'; c.fillRect((t / 1000 * 300) % 300, 10, 20, 40);
  document.getElementById('t').textContent = Math.round(t) + ' ms';
  requestAnimationFrame(draw);
}
requestAnimationFrame(draw);
</script></body></html>"#;

/// A page made only to be recorded: it shows the app it belongs to, from
/// the SDK's tag, and fails without it.
const RENDER: &str = r#"<!doctype html><html><head></head><body><h1 id="h">render</h1><script>
var tag = document.querySelector('meta[name="nebo-app-id"]');
if (!tag) throw new Error('render.html has no nebo-app-id tag');
document.getElementById('h').textContent = tag.content;
</script></body></html>"#;

const BROKEN: &str = r#"<!doctype html><html><head></head><body><h1>Broken</h1><script>
startTheShow();
</script></body></html>"#;

struct Rig {
    _server: TestServer,
    data_dir: PathBuf,
    record: tools::app_record::AppRecordTool,
    shot: tools::app_publish::AppScreenshotTool,
    ctx: tools::ToolContext,
}

async fn rig() -> Option<Rig> {
    let server = TestServer::boot().await;
    // The port the tools address the bot on (`napp::plugin::local_port`).
    // SAFETY: one test runs in this binary at a time; nothing else reads it yet.
    unsafe { std::env::set_var("NEBO_PORT", server.port.to_string()) };
    if browser::chrome::find_chrome().is_none() {
        eprintln!("no Chrome or Chromium on this machine — skipping");
        return None;
    }
    let manager = Arc::new(browser::Manager::new(
        browser::BrowserConfig::default(),
        server.data_dir.to_string_lossy().into_owned(),
    ));
    let ui = server.data_dir.join("apps").join("motion").join("ui");
    std::fs::create_dir_all(&ui).unwrap();
    std::fs::write(ui.join("index.html"), "<!doctype html><html><head></head><body><h1>Motion</h1></body></html>").unwrap();
    std::fs::write(ui.join("anim.html"), ANIMATED).unwrap();
    std::fs::write(ui.join("render.html"), RENDER).unwrap();
    std::fs::write(ui.join("broken.html"), BROKEN).unwrap();
    let store = Arc::new(server.db_store());
    store
        .create_agent("app-motion", None, "Motion", "Motion posts", "", "", None, None)
        .unwrap();
    store
        .set_agent_app_fields("app-motion", true, Some(ui.to_str().unwrap()), None, None)
        .unwrap();
    Some(Rig {
        data_dir: server.data_dir.clone(),
        record: tools::app_record::AppRecordTool::new(store.clone(), Some(manager.clone())),
        shot: tools::app_publish::AppScreenshotTool::new(store, Some(manager)),
        ctx: tools::ToolContext {
            origin: tools::origin::Origin::User,
            session_key: "agent:app-motion:web".into(),
            ..Default::default()
        },
        _server: server,
    })
}

/// The recordings made so far, oldest first.
fn recordings(data_dir: &Path) -> Vec<PathBuf> {
    let root = data_dir.join("files").join("app-recordings").join("Motion");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    dirs.sort();
    dirs
}

fn frames(dir: &Path) -> Vec<Vec<u8>> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".png"))
        .collect();
    names.sort();
    names.iter().map(|n| std::fs::read(dir.join(n)).unwrap()).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_recording_steps_the_pages_clock_and_a_failed_page_is_an_error() {
    let Some(rig) = rig().await else { return };

    // fps × seconds frames, every one its own moment.
    let input = json!({ "path": "anim.html", "width": 320, "height": 240, "seconds": 1, "fps": 12 });
    let first = rig.record.execute_dyn(&rig.ctx, input.clone()).await;
    assert!(!first.is_error, "{}", first.content);
    assert!(first.content.contains("Recorded 12 frames"), "{}", first.content);
    let dir = recordings(&rig.data_dir).pop().expect("the recording's folder");
    let shots = frames(&dir);
    assert_eq!(shots.len(), 12, "fps × seconds frames");
    for n in 1..=12 {
        assert!(dir.join(format!("frame_{n:05}.png")).is_file(), "frame_{n:05}.png");
    }
    for pair in shots.windows(2) {
        assert_ne!(pair[0], pair[1], "each frame is a later moment than the one before");
    }
    let manifest: Value = serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest, json!({ "width": 320, "height": 240, "fps": 12, "frames": 12 }));
    assert_eq!(ai::image_norm::dimensions(&shots[0]), Some((320, 240)));

    // The same page recorded again is the same frames: time was stepped,
    // never read off the machine's clock.
    let again = rig.record.execute_dyn(&rig.ctx, input).await;
    assert!(!again.is_error, "{}", again.content);
    let second = recordings(&rig.data_dir).pop().unwrap();
    assert_ne!(second, dir);
    assert!(frames(&second) == shots, "a second recording draws the same frames");

    // render.html carries the SDK's tags: the page finds its app and draws.
    let render = rig
        .shot
        .execute_dyn(&rig.ctx, json!({ "path": "render.html", "width": 320, "height": 240, "wait_ms": 800 }))
        .await;
    assert!(!render.is_error, "{}", render.content);

    // A page that throws is an error from both tools, with what it threw;
    // a failed recording leaves nothing behind.
    let before = recordings(&rig.data_dir).len();
    let failed = rig
        .record
        .execute_dyn(&rig.ctx, json!({ "path": "broken.html", "width": 320, "height": 240, "seconds": 1 }))
        .await;
    assert!(failed.is_error, "{}", failed.content);
    assert!(failed.content.contains("startTheShow"), "{}", failed.content);
    assert_eq!(recordings(&rig.data_dir).len(), before, "no partial frames are kept");
    let shot = rig
        .shot
        .execute_dyn(&rig.ctx, json!({ "path": "broken.html", "width": 320, "height": 240, "wait_ms": 800 }))
        .await;
    assert!(shot.is_error, "{}", shot.content);
    assert!(shot.content.contains("startTheShow"), "{}", shot.content);
    assert!(shot.image_url.is_none(), "no picture of a failed page");

    // A page the app does not have is said, never shown as the entry page.
    let missing = rig.shot.execute_dyn(&rig.ctx, json!({ "path": "nope.html" })).await;
    assert!(missing.is_error && missing.content.contains("has no page nope.html"), "{}", missing.content);

    // The bounds hold.
    let long = rig.record.execute_dyn(&rig.ctx, json!({ "path": "anim.html", "seconds": 61 })).await;
    assert!(long.is_error, "{}", long.content);
    let fast = rig.record.execute_dyn(&rig.ctx, json!({ "path": "anim.html", "seconds": 1, "fps": 61 })).await;
    assert!(fast.is_error, "{}", fast.content);
}

/// The end-to-end check by hand: a 10-second 1080×1080 post at 30 fps,
/// copied to `APP_RECORD_OUT` for Nebo Media's `video encode`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "by hand: APP_RECORD_OUT=<folder> cargo test -p nebo-server --test app_record -- --ignored"]
async fn a_ten_second_post_for_video_encode() {
    let out = PathBuf::from(std::env::var("APP_RECORD_OUT").expect("APP_RECORD_OUT"));
    let Some(rig) = rig().await else { return };
    let started = std::time::Instant::now();
    let r = rig
        .record
        .execute_dyn(&rig.ctx, json!({ "path": "anim.html", "width": 1080, "height": 1080, "seconds": 10 }))
        .await;
    assert!(!r.is_error, "{}", r.content);
    eprintln!("{} ({:.1} s)", r.content, started.elapsed().as_secs_f64());
    let dir = recordings(&rig.data_dir).pop().unwrap();
    assert_eq!(frames(&dir).len(), 300);
    std::fs::create_dir_all(&out).unwrap();
    for e in std::fs::read_dir(&dir).unwrap().flatten() {
        std::fs::copy(e.path(), out.join(e.file_name())).unwrap();
    }
}
