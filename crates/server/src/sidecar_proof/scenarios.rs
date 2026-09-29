//! Each scenario is one way an app's sidecar stops serving, and the proof
//! that the app is serving again, or says exactly why not.

use std::time::{Duration, Instant};

use napp::supervisor::{RestartPolicy, SidecarState};
use napp::test_sidecar;

use super::{World, quick};

/// Killed out from under Nebo (an OOM kill, a stray `kill -9`): the exit is
/// awaited, noticed at once, and the next request is served by a new process
/// within seconds — no Nebo restart, no "isn't running".
#[tokio::test]
async fn a_killed_sidecar_serves_the_next_request_within_seconds() {
    let w = World::new("sc-killed", quick(), |_| {}).await;
    assert_eq!(w.get("me").await.expect("served"), "GET me");
    let first = w.pid();
    test_sidecar::kill(first);
    let t = Instant::now();
    assert_eq!(w.get("me").await.expect("served again"), "GET me");
    assert!(t.elapsed() < Duration::from_secs(5), "served again in {:?}", t.elapsed());
    assert_ne!(w.pid(), first, "by a new process");
    assert!(!test_sidecar::exists(first), "the dead one was reaped");
    w.lifecycle.shutdown().await;
}

/// The owner's report, "isn't running" until Nebo restarts: a process behind
/// a leftover socket file that refuses every connection. The request that
/// cannot reach it revives it through the supervisor and is answered —
/// nothing keys on the socket file existing.
#[tokio::test]
async fn a_stale_socket_and_a_dead_listener_are_served_by_relaunching() {
    // The probe far away, so it is the request that finds it.
    let policy = RestartPolicy { probe_every: Duration::from_secs(3600), ..quick() };
    let w = World::new("sc-stale", policy, |app| app.set_mode("deaf")).await;
    let first = match w.lifecycle.settled(Duration::from_secs(20)).await {
        SidecarState::Running(l) => l.pid,
        other => panic!("expected running, got {other:?}"),
    };
    assert!(w.app.sock_path().exists(), "the socket file is there");
    w.app.set_mode("serve");
    assert_eq!(w.get("me").await.expect("served by the relaunched one"), "GET me");
    assert_ne!(w.pid(), first);
    w.lifecycle.shutdown().await;
}

/// A sidecar that crashes on every start is not relaunched forever in a tight
/// loop and never flaps: after a few fast attempts it is `failed`, with the
/// exit status and its own last words as the reason the app shows, and "Try
/// again" brings it back once it is fixed.
#[tokio::test]
async fn a_crash_loop_ends_failed_with_its_reason() {
    let w = World::new("sc-crashy", quick(), |app| app.set_mode("exit 3")).await;
    w.until(20, |s| matches!(s, SidecarState::Failed { .. })).await;
    let unavailable = w.get("me").await.expect_err("not served while failed");
    let shown = unavailable.message("Crashy");
    assert!(shown.starts_with("Crashy stopped: "), "{shown}");
    assert!(shown.contains("exited with code 3"), "{shown}");
    assert!(shown.contains("fixture: exiting with 3"), "the sidecar's last output: {shown}");
    assert!(!shown.contains("could not be reached"), "never the generic line: {shown}");
    assert_eq!(unavailable.state.wire()["state"], "failed");

    w.app.set_mode("serve");
    assert!(w.lifecycle.revive(Duration::from_secs(20)).await.is_running(), "Try again");
    assert_eq!(w.get("me").await.expect("served"), "GET me");
    w.lifecycle.shutdown().await;
}

/// A program missing at boot is a permanent cause: the app shows why and to
/// reinstall, instead of a generic failure — and the supervisor, still
/// checking on its slow cadence, has the app serving as soon as the program
/// is back.
#[tokio::test]
async fn a_boot_failure_recovers_when_the_program_appears() {
    let w = World::new("sc-absent", quick(), |app| app.remove_binary()).await;
    let SidecarState::Failed { permanent, .. } = w.lifecycle.settled(Duration::from_secs(20)).await else {
        panic!("expected failed: {:?}", w.lifecycle.state());
    };
    assert!(permanent);
    let shown = w.get("me").await.expect_err("nothing to serve").message("Absent");
    assert!(shown.contains("can't run on this computer") && shown.contains("Reinstall Absent"), "{shown}");

    w.app.install_binary();
    w.until(10, SidecarState::is_running).await;
    assert_eq!(w.get("me").await.expect("served"), "GET me");
    w.lifecycle.shutdown().await;
}

/// Stopping on purpose (Nebo exiting, a hot reload, the app deactivated) is
/// not a crash: the app is announced stopped, never crashed or restarting,
/// the process is gone and nothing relaunches it.
#[tokio::test]
async fn a_clean_shutdown_is_not_a_crash() {
    let w = World::new("sc-clean", quick(), |_| {}).await;
    assert_eq!(w.get("me").await.expect("served"), "GET me");
    let pid = w.pid();
    let mut events = w.hub.subscribe();
    w.lifecycle.shutdown().await;
    let mut seen = Vec::new();
    while let Ok(e) = events.try_recv() {
        seen.push((e.event_type.clone(), e.payload["state"].as_str().unwrap_or("").to_string()));
    }
    assert!(seen.iter().any(|(t, _)| t == "app_stopped"), "{seen:?}");
    assert!(seen.iter().any(|(t, s)| t == "sidecar_state" && s == "off"), "{seen:?}");
    assert!(!seen.iter().any(|(t, s)| t == "app_crashed" || s == "restarting" || s == "failed"), "{seen:?}");
    assert!(!test_sidecar::exists(pid));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(w.app.launches(), 1, "nothing relaunched it");
}

/// Each app keeps its data in its own folder, keyed by its agent id — never
/// the folder named after its code folder's parent that every app under
/// `user/agents/` used to share. Its `sidecar.log` is there too.
#[tokio::test]
async fn each_app_keeps_its_data_in_its_own_folder() {
    let w = World::new("sc-own-data", quick(), |_| {}).await;
    assert_eq!(w.get("me").await.expect("served"), "GET me");
    let own = w.app.home.join("appdata/agents/sc-own-data");
    assert!(own.join("sidecar.log").is_file(), "the sidecar's log is in {}", own.display());
    assert!(!w.app.home.join("appdata/plugins/agents").exists(), "nothing in the old shared folder");
    w.lifecycle.shutdown().await;
}
