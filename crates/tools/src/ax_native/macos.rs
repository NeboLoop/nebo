//! macOS backend: the embedded `ax_helper.swift`, compiled on first use.

use super::WalkOpts;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncReadExt;

const SOURCE: &str = include_str!("ax_helper.swift");
const FRAMEWORKS: &[&str] = &["ApplicationServices", "AppKit", "Vision"];
const ACTION_DEADLINE: Duration = Duration::from_secs(5);

async fn helper() -> Result<PathBuf, String> {
    crate::swift_helper::ensure_compiled("ax-helper", SOURCE, FRAMEWORKS)
        .await
        .ok_or_else(|| "native accessibility helper unavailable (swiftc missing or compile failed)".to_string())
}

fn tree_args(app: &str, opts: &WalkOpts) -> Vec<String> {
    let mut v: Vec<String> = vec![
        "tree".into(),
        "--app".into(),
        app.into(),
        "--window".into(),
        opts.window.to_string(),
        "--depth".into(),
        opts.depth.to_string(),
        "--max".into(),
        opts.max.to_string(),
        "--timeout-ms".into(),
        opts.timeout.as_millis().to_string(),
    ];
    if let Some(root) = opts.root.as_ref().filter(|r| !r.is_empty()) {
        v.push("--root".into());
        v.push(root.clone());
    }
    v
}

pub(super) async fn run(args: &[String], deadline: Duration) -> Result<String, String> {
    let bin = helper().await?;
    run_cmd(&bin, args, deadline).await
}

/// Run `program` and return its stdout. Past `deadline` the process is
/// killed and whatever it had written is returned if it got as far as the
/// header — a partial walk is still a walk — otherwise an error.
async fn run_cmd(program: &Path, args: &[String], deadline: Duration) -> Result<String, String> {
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("could not start {}: {e}", program.display()))?;
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let out = tokio::spawn(async move {
        let mut b = Vec::new();
        let _ = stdout.read_to_end(&mut b).await;
        b
    });
    let err = tokio::spawn(async move {
        let mut b = Vec::new();
        let _ = stderr.read_to_end(&mut b).await;
        b
    });
    let status = tokio::time::timeout(deadline, child.wait()).await;
    let collect = |t: tokio::task::JoinHandle<Vec<u8>>| async move {
        String::from_utf8_lossy(&t.await.unwrap_or_default()).into_owned()
    };
    match status {
        Ok(Ok(st)) if st.success() => Ok(collect(out).await),
        Ok(Ok(st)) => {
            let e = collect(err).await;
            let e = e.trim();
            Err(if e.is_empty() { format!("ax-helper exited with {st}") } else { e.to_string() })
        }
        Ok(Err(e)) => Err(format!("ax-helper failed: {e}")),
        Err(_) => {
            let _ = child.kill().await;
            let partial = collect(out).await;
            if partial.lines().next().map_or(false, |l| l.contains("\"pid\"")) {
                Ok(partial)
            } else {
                // Named: which call, which budget.
                Err(format!(
                    "ax-helper {} passed its {} ms deadline and was stopped; the app may be busy",
                    args.first().map(String::as_str).unwrap_or("call"),
                    deadline.as_millis()
                ))
            }
        }
    }
}

pub(super) async fn tree_raw(app: &str, opts: &WalkOpts) -> Result<String, String> {
    let bin = helper().await?;
    run_cmd(&bin, &tree_args(app, opts), opts.timeout + Duration::from_millis(500)).await
}

pub(super) async fn window_raw(app: &str, index: usize) -> Result<String, String> {
    let program = helper().await?;
    run_cmd(
        &program,
        &["window".to_string(), "--app".to_string(), app.to_string(), "--window".to_string(), index.to_string()],
        ACTION_DEADLINE,
    )
    .await
}

/// Text in an image, via the Vision framework in the same helper.
pub(super) async fn text_raw(image: &Path) -> Result<String, String> {
    let program = helper().await?;
    run_cmd(
        &program,
        &["text".to_string(), "--image".to_string(), image.to_string_lossy().into_owned()],
        Duration::from_secs(10),
    )
    .await
}

/// `act`/`set` arguments; `expect` adds the role and label the helper must
/// re-identify before acting.
fn target_args(cmd: &str, app: &str, window: usize, path: &str, key: &str, val: &str, expect: super::Expect<'_>) -> Vec<String> {
    let mut a: Vec<String> = [cmd, "--app", app, "--window", &window.to_string(), "--path", path, key, val]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if let Some((role, label)) = expect {
        a.extend(["--role", role, "--label", label].map(String::from));
    }
    a
}

pub(super) async fn act_raw(app: &str, window: usize, path: &str, action: &str, expect: super::Expect<'_>) -> Result<(), String> {
    let bin = helper().await?;
    run_cmd(&bin, &target_args("act", app, window, path, "--action", action, expect), ACTION_DEADLINE).await.map(|_| ())
}

pub(super) async fn set_raw(app: &str, window: usize, path: &str, value: &str, expect: super::Expect<'_>) -> Result<(), String> {
    let bin = helper().await?;
    run_cmd(&bin, &target_args("set", app, window, path, "--value", value, expect), ACTION_DEADLINE).await.map(|_| ())
}

pub(super) async fn frontmost_raw() -> Result<String, String> {
    let bin = helper().await?;
    run_cmd(&bin, &["frontmost".to_string()], ACTION_DEADLINE).await
}

pub(super) async fn raise_raw(app: &str, window: usize) -> Result<(), String> {
    let bin = helper().await?;
    let args: Vec<String> = ["raise", "--app", app, "--window", &window.to_string()].map(String::from).to_vec();
    run_cmd(&bin, &args, ACTION_DEADLINE).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SH: &str = "/bin/sh";
    fn sh(script: &str) -> Vec<String> {
        vec!["-c".into(), script.into()]
    }

    #[test]
    fn an_expected_role_and_label_ride_along_and_their_absence_means_the_path_alone() {
        let a = target_args("act", "Simulator", 1, "0.3.2", "--action", "AXPress", Some(("AXButton", "Continue with email")));
        assert!(a.ends_with(&["--role".into(), "AXButton".into(), "--label".into(), "Continue with email".into()]), "{a:?}");
        let b = target_args("set", "Simulator", 1, "0.3.2", "--value", "716917", None);
        assert!(!b.iter().any(|x| x == "--role"), "{b:?}");
        assert_eq!(&b[..3], &["set", "--app", "Simulator"]);
    }

    #[test]
    fn tree_args_carry_every_budget() {
        let opts = WalkOpts { window: 2, depth: 7, max: 50, timeout: Duration::from_millis(1500), root: None };
        let a = tree_args("Finder", &opts);
        assert_eq!(a[..3], ["tree", "--app", "Finder"]);
        for (flag, val) in [("--window", "2"), ("--depth", "7"), ("--max", "50"), ("--timeout-ms", "1500")] {
            let i = a.iter().position(|x| x == flag).unwrap_or_else(|| panic!("{flag} missing"));
            assert_eq!(a[i + 1], val, "{flag}");
        }
    }

    #[tokio::test]
    async fn a_timed_out_walk_keeps_its_partial_output_when_the_header_arrived() {
        let out = run_cmd(
            Path::new(SH),
            &sh(r#"echo '{"app":"X","pid":1,"windows":1}'; echo '{"path":"0","role":"AXButton","frame":[0,0,1,1]}'; sleep 5"#),
            Duration::from_millis(300),
        )
        .await
        .unwrap();
        assert_eq!(out.lines().count(), 2);
        assert!(out.contains("AXButton"));
    }

    #[tokio::test]
    async fn a_timed_out_walk_with_no_header_is_an_error() {
        let err = run_cmd(Path::new(SH), &sh("sleep 5"), Duration::from_millis(200)).await.unwrap_err();
        assert!(err.contains("passed its 200 ms deadline"), "the deadline is named: {err}");
    }

    #[tokio::test]
    async fn a_failing_helper_reports_its_stderr() {
        let err = run_cmd(Path::new(SH), &sh("echo 'no window 3' >&2; exit 1"), Duration::from_secs(2)).await.unwrap_err();
        assert_eq!(err, "no window 3");
    }

    #[tokio::test]
    async fn a_missing_program_is_an_error_not_a_panic() {
        assert!(run_cmd(Path::new("/nonexistent/ax-helper"), &[], Duration::from_secs(1)).await.is_err());
    }

    /// Read-only: walks the running Finder. `cargo test -p nebo-tools --lib ax_native -- --ignored`
    #[tokio::test]
    #[ignore]
    async fn live_finder_walk_has_pressable_nodes() {
        let t = super::super::tree("Finder", &WalkOpts::default()).await.unwrap();
        assert_eq!(t.app, "Finder");
        assert!(t.nodes.len() >= 3, "got {} nodes", t.nodes.len());
        // Finder with no window open walks the Desktop, whose icons offer
        // AXShowMenu rather than AXPress; any action proves the walk is live.
        assert!(t.nodes.iter().any(|n| !n.actions.is_empty()), "no actionable node");
        eprintln!("Finder: {} nodes in {} ms, truncated={}", t.nodes.len(), t.elapsed_ms, t.truncated);
    }
}
