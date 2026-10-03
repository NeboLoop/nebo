//! End to end: drive the plugin binary the way Nebo does (JSON on stdin) with
//! an empty environment, so nothing can come from a system ffmpeg.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::read::GzDecoder;
use serde_json::{Value, json};

/// Unpacks the same gzipped ffmpeg the plugin embeds, to make fixtures with.
fn fixture_ffmpeg(dir: &Path) -> PathBuf {
    let exe = if cfg!(windows) { "fixture-ffmpeg.exe" } else { "fixture-ffmpeg" };
    let path = dir.join(exe);
    let gz = fs::File::open(env!("NEBO_VIDEO_FFMPEG")).unwrap();
    let mut out = fs::File::create(&path).unwrap();
    std::io::copy(&mut GzDecoder::new(gz), &mut out).unwrap();
    drop(out);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

fn scratch() -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("nebo-video-e2e-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn plugin(data_dir: &Path, action: &str, input: Value) -> Value {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_nebo-video"));
    cmd.arg(action)
        .env_clear()
        .env("NEBO_DATA_DIR", data_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Windows cannot start processes without SystemRoot.
    if let Some(root) = std::env::var_os("SYSTEMROOT") {
        cmd.env("SYSTEMROOT", root);
    }
    let mut child = cmd.spawn().expect("spawn plugin");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{action}: non-JSON stdout ({e}): {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert!(out.status.success(), "{action} failed: {v}");
    v
}

fn fixture(ffmpeg: &Path, args: &[&str]) {
    let status = Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "fixture ffmpeg {args:?}");
}

fn approx(v: &Value, want: f64, tol: f64) {
    let got = v.as_f64().unwrap_or_else(|| panic!("not a number: {v}"));
    assert!((got - want).abs() <= tol, "expected {want}±{tol}, got {got}");
}

#[test]
fn probe_trim_render_with_no_system_ffmpeg() {
    let dir = scratch();
    let data = dir.join("data");
    let s = |name: &str| dir.join(name).to_string_lossy().into_owned();
    let ffmpeg = fixture_ffmpeg(&dir);

    // 3 s of colour bars with a tone, and a separate 2 s tone.
    fixture(&ffmpeg, &[
        "-f", "lavfi", "-i", "testsrc=duration=3:size=640x360:rate=30",
        "-f", "lavfi", "-i", "sine=frequency=440:duration=3",
        "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest", &s("a.mp4"),
    ]);
    fixture(&ffmpeg, &["-f", "lavfi", "-i", "sine=frequency=660:duration=2", &s("b.wav")]);

    let a = plugin(&data, "probe", json!({ "input": s("a.mp4") }));
    approx(&a["duration"], 3.0, 0.1);
    assert_eq!(a["width"], 640);
    assert_eq!(a["height"], 360);
    assert_eq!(a["video_codec"], "h264");
    assert_eq!(a["audio_codec"], "aac");

    let cut = plugin(
        &data,
        "trim",
        json!({ "input": s("a.mp4"), "start": 0.5, "end": 2.5, "output": s("cut.mp4"), "reencode": true }),
    );
    assert_eq!(cut["mode"], "reencode");
    approx(&plugin(&data, "probe", json!({ "input": s("cut.mp4") }))["duration"], 2.0, 0.1);

    // Every character the filter graph treats specially, in one overlay.
    let project = json!({
        "output": { "width": 1280, "height": 720, "fps": 30, "codec": "h264" },
        "tracks": [
            { "type": "video", "clips": [
                { "source": s("a.mp4"),   "start": 0.0, "duration": 1.5, "trim_start": 0.0 },
                { "source": s("cut.mp4"), "start": 1.5, "duration": 1.0, "trim_start": 0.0 }
            ]},
            { "type": "audio", "clips": [
                { "source": s("b.wav"), "start": 0.25, "duration": 2.0, "volume": 0.8 }
            ]},
            { "type": "text", "clips": [
                { "text": "It's 50% off: today, only [now]; \\o/", "start": 0.2, "duration": 2.0,
                  "size": 48, "x": 0.5, "y": 0.8 }
            ]}
        ]
    });
    fs::write(dir.join("project.json"), project.to_string()).unwrap();
    plugin(&data, "render", json!({ "project": s("project.json"), "output": s("out.mp4") }));

    let out = plugin(&data, "probe", json!({ "input": s("out.mp4") }));
    approx(&out["duration"], 2.5, 0.15);
    assert_eq!(out["width"], 1280);
    assert_eq!(out["height"], 720);
    assert_eq!(out["video_codec"], "h264");
    assert_eq!(out["audio_codec"], "aac");

    // ffmpeg was extracted into the plugin's data dir, not found elsewhere.
    let installed: Vec<_> = fs::read_dir(data.join("ffmpeg"))
        .unwrap()
        .flat_map(|d| fs::read_dir(d.unwrap().path()).unwrap())
        .map(|f| f.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    for name in ["ffmpeg", "ffprobe", "Inter-Regular.ttf"] {
        assert!(
            installed.iter().any(|f| f.trim_end_matches(".exe") == name),
            "{name} not extracted: {installed:?}"
        );
    }

    fs::remove_dir_all(&dir).unwrap();
}
