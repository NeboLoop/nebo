//! Linux backend: AT-SPI2 through an embedded python3 script.
//!
//! The script is fed to `python3 -` on stdin per call. No file in the data
//! dir, no hash, nothing to go stale after an update.
//! ponytail: stdin instead of a cached script file; switch to a file only if
//! the ~10 KB write per call ever shows up in a profile.

use super::WalkOpts;
use std::time::Duration;

const SCRIPT: &str = include_str!("ax_helper_atspi.py");

/// PowerShell-free equivalent of the macOS helper's startup cost: python3
/// plus gi import. Added to the walk budget so a slow start is not reported
/// as a slow walk.
const STARTUP_GRACE: Duration = Duration::from_secs(3);

pub(super) async fn tree_raw(app: &str, opts: &WalkOpts) -> Result<String, String> {
    run(&tree_args(app, opts), opts.timeout + STARTUP_GRACE).await
}

pub(super) async fn act_raw(app: &str, window: usize, path: &str, action: &str, _expect: super::Expect<'_>) -> Result<(), String> {
    let w = window.to_string();
    run(
        &["act", "--app", app, "--window", &w, "--path", path, "--action", action].map(String::from),
        Duration::from_secs(5) + STARTUP_GRACE,
    )
    .await
    .map(|_| ())
}

pub(super) async fn set_raw(app: &str, window: usize, path: &str, value: &str, _expect: super::Expect<'_>) -> Result<(), String> {
    let w = window.to_string();
    run(
        &["set", "--app", app, "--window", &w, "--path", path, "--value", value].map(String::from),
        Duration::from_secs(5) + STARTUP_GRACE,
    )
    .await
    .map(|_| ())
}

/// Text in an image via tesseract's tsv output, grouped into lines here.
pub(super) async fn text_raw(image: &std::path::Path) -> Result<String, String> {
    let out = tokio::time::timeout(
        Duration::from_secs(20),
        tokio::process::Command::new("tesseract")
            .arg(image)
            .arg("-")
            .arg("tsv")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| "tesseract took longer than 20s".to_string())?
    .map_err(|e| format!("tesseract is not installed ({e}); the cloud image ships it, a dev box needs `apt install tesseract-ocr`"))?;
    if !out.status.success() {
        return Err(format!("tesseract failed: {}", failure_text(out.status.code(), &String::from_utf8_lossy(&out.stderr))));
    }
    let lines = super::tesseract_tsv_to_lines(&String::from_utf8_lossy(&out.stdout));
    Ok(lines
        .iter()
        .map(|l| serde_json::json!({ "text": l.text, "frame": l.frame, "confidence": l.confidence }).to_string())
        .collect::<Vec<_>>()
        .join("\n"))
}

fn tree_args(app: &str, opts: &WalkOpts) -> Vec<String> {
    [
        "tree",
        "--app",
        app,
        "--window",
        &opts.window.to_string(),
        "--depth",
        &opts.depth.to_string(),
        "--max",
        &opts.max.to_string(),
        "--timeout-ms",
        &opts.timeout.as_millis().to_string(),
    ]
    .map(String::from)
    .to_vec()
}

/// What a failed run is reported as: the script's own last words when it
/// had any, the exit status when it did not.
fn failure_text(code: Option<i32>, stderr: &str) -> String {
    let last = stderr.trim().lines().last().unwrap_or("").trim();
    if last.is_empty() {
        format!("ax helper exited with status {}", code.map_or("signal".to_string(), |c| c.to_string()))
    } else {
        last.to_string()
    }
}

async fn run(args: &[String], timeout: Duration) -> Result<String, String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    let mut cmd = tokio::process::Command::new("python3");
    cmd.arg("-")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(display) = crate::desktop_session::display() {
        crate::desktop_session::touch();
        cmd.env("DISPLAY", display);
    }
    let mut child = cmd.spawn().map_err(|e| format!("python3 is not available: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(SCRIPT.as_bytes())
            .await
            .map_err(|e| format!("could not hand the ax script to python3: {e}"))?;
    }
    let out = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| format!("ax helper did not finish within {}s", timeout.as_secs()))?
        .map_err(|e| format!("ax helper failed to run: {e}"))?;
    if !out.status.success() {
        return Err(failure_text(out.status.code(), &String::from_utf8_lossy(&out.stderr)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_args_carry_every_budget() {
        let opts = WalkOpts { window: 2, depth: 5, max: 50, timeout: Duration::from_millis(1500) };
        assert_eq!(
            tree_args("Firefox", &opts),
            ["tree", "--app", "Firefox", "--window", "2", "--depth", "5", "--max", "50", "--timeout-ms", "1500"]
        );
    }

    #[test]
    fn failure_text_prefers_the_scripts_last_line() {
        assert_eq!(
            failure_text(Some(1), "Traceback\n  ...\nSystemExit: no application named 'x'\n"),
            "SystemExit: no application named 'x'"
        );
        assert_eq!(failure_text(Some(2), "  \n"), "ax helper exited with status 2");
        assert_eq!(failure_text(None, ""), "ax helper exited with status signal");
    }
}
