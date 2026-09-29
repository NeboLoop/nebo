//! The one supervisor for a sidecar process. It is the only thing that starts,
//! watches and restarts one: boot, an on-request start, "Try again", a binary
//! rebuilt on disk and a dead or silent process all go through it.
//!
//! Liveness is active. The child's exit is awaited on its own handle (never
//! inferred from `kill(pid, 0)`, which a zombie still answers), and its socket
//! is probed on a timer, so a process that died, or stopped accepting
//! connections, is noticed within seconds instead of on the next request.
//!
//! A failure is logged with its exit status and the last lines of the
//! sidecar's log, its socket is removed, and it is restarted with backoff. A
//! run of fast failures ends in [`SidecarState::Failed`], the state the app
//! shows the person, after which it keeps retrying on the slow cadence without
//! flapping the state. A permanent cause (no program, one the system refuses
//! to run, a damaged manifest) is `Failed` at once. A clean shutdown is never
//! counted as a crash.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot, watch};
use tracing::{info, warn};

use crate::manifest::Manifest;
use crate::runtime::{Process, Runtime};

/// How a supervisor waits between attempts and decides a sidecar is down.
#[derive(Debug, Clone)]
pub struct RestartPolicy {
    /// Wait before the first restart; doubles on each consecutive failure.
    pub first_delay: Duration,
    /// The longest wait between attempts, and the slow cadence once `Failed`.
    pub max_delay: Duration,
    /// A sidecar that served this long is healthy: its failure count resets.
    pub healthy_after: Duration,
    /// Consecutive failures before the app shows `Failed`.
    pub max_failures: u32,
    /// How often a running sidecar's socket is probed.
    pub probe_every: Duration,
    /// Consecutive failed probes that mean the process is no longer serving.
    pub probe_misses: u32,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            first_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            healthy_after: Duration::from_secs(60),
            max_failures: 5,
            probe_every: Duration::from_secs(3),
            probe_misses: 3,
        }
    }
}

impl RestartPolicy {
    /// The wait after the `failures`-th consecutive failure (1-based).
    pub fn delay(&self, failures: u32) -> Duration {
        let doublings = failures.saturating_sub(1).min(16);
        self.first_delay.saturating_mul(1 << doublings).min(self.max_delay)
    }
}

/// A launched sidecar: where to reach it and what it was given.
#[derive(Debug)]
pub struct Launched {
    pub pid: u32,
    pub sock_path: PathBuf,
    /// The per-launch token the sidecar authenticates to Nebo's API with.
    pub app_token: String,
    pub manifest: Manifest,
}

/// What a supervised sidecar is doing. Every change is published.
#[derive(Debug, Clone)]
pub enum SidecarState {
    /// Being launched.
    Starting,
    /// Serving.
    Running(Arc<Launched>),
    /// Went down and will be launched again after `retry_in`.
    Restarting { attempt: u32, retry_in: Duration, reason: String },
    /// Not running and out of fast retries, or down for a cause retrying cannot
    /// fix (`permanent`). The app shows this; "Try again" relaunches at once,
    /// and the supervisor still retries on the slow cadence.
    Failed { reason: String, permanent: bool },
    /// Stopped on purpose (Nebo shutting down, the app deactivated or
    /// replaced). Not a failure.
    Off,
}

impl SidecarState {
    pub fn is_running(&self) -> bool {
        matches!(self, SidecarState::Running(_))
    }

    /// Running, failed or off: nothing is in flight.
    pub fn is_settled(&self) -> bool {
        matches!(self, SidecarState::Running(_) | SidecarState::Failed { .. } | SidecarState::Off)
    }

    /// The state as the app and the WebSocket see it.
    pub fn wire(&self) -> serde_json::Value {
        use serde_json::json;
        match self {
            SidecarState::Starting => json!({ "state": "starting" }),
            SidecarState::Running(l) => json!({ "state": "running", "pid": l.pid }),
            SidecarState::Restarting { attempt, retry_in, reason } => json!({
                "state": "restarting",
                "attempt": attempt,
                "retryInMs": retry_in.as_millis() as u64,
                "reason": reason,
            }),
            SidecarState::Failed { reason, permanent } => json!({
                "state": "failed",
                "reason": reason,
                "permanent": permanent,
            }),
            SidecarState::Off => json!({ "state": "off" }),
        }
    }
}

enum Cmd {
    /// Bring it up now: a request found it unreachable, or the person pressed
    /// "Try again". A running sidecar is probed; a waiting one launches now; a
    /// failed one starts over with a fresh failure count. Answered with the
    /// next settled state.
    Revive(oneshot::Sender<SidecarState>),
    /// Stop it for good. Not a failure.
    Stop,
}

/// Supervises one sidecar for its whole life. Dropping it without
/// [`Supervisor::shutdown`] still stops the process (its handle kills on drop).
pub struct Supervisor {
    state: watch::Receiver<SidecarState>,
    cmds: mpsc::UnboundedSender<Cmd>,
}

impl Supervisor {
    /// Start supervising the sidecar in `tool_dir`. Returns at once; the first
    /// launch happens in the background and is published like every other
    /// state change.
    pub fn start(runtime: Arc<Runtime>, tool_dir: PathBuf, api_port: u16, policy: RestartPolicy) -> Self {
        let (tx, state) = watch::channel(SidecarState::Starting);
        let (cmds, rx) = mpsc::unbounded_channel();
        tokio::spawn(supervise(runtime, tool_dir, api_port, policy, tx, rx));
        Self { state, cmds }
    }

    pub fn state(&self) -> SidecarState {
        self.state.borrow().clone()
    }

    /// Every state change, as it happens.
    pub fn subscribe(&self) -> watch::Receiver<SidecarState> {
        self.state.clone()
    }

    /// Ask for the sidecar to be up now, the same path a crash takes: a
    /// running one is probed and relaunched at once if it does not answer, a
    /// waiting one launches now, a failed one starts over. Waits up to `wait`
    /// for the outcome and returns the state then.
    pub async fn revive(&self, wait: Duration) -> SidecarState {
        let (reply, answer) = oneshot::channel();
        if self.cmds.send(Cmd::Revive(reply)).is_err() {
            return SidecarState::Off;
        }
        match tokio::time::timeout(wait, answer).await {
            Ok(Ok(s)) => s,
            _ => self.state(),
        }
    }

    /// Wait up to `wait` for a settled state (running, failed or off). A
    /// sidecar still starting or restarting when the time is up returns that.
    pub async fn settled(&self, wait: Duration) -> SidecarState {
        let mut rx = self.state.clone();
        let _ = tokio::time::timeout(wait, rx.wait_for(SidecarState::is_settled)).await;
        self.state()
    }

    /// Stop the sidecar and wait for it to be gone. Not counted as a crash.
    pub async fn shutdown(&self) {
        let _ = self.cmds.send(Cmd::Stop);
        let mut rx = self.state.clone();
        // Off is published once the process is stopped and its socket removed;
        // a closed channel means the supervisor is already gone.
        let _ = rx.wait_for(|s| matches!(s, SidecarState::Off)).await;
    }
}

/// Publishes state changes and answers the revive requests waiting on the
/// next settled state.
struct Publisher {
    tx: watch::Sender<SidecarState>,
    waiting: Vec<oneshot::Sender<SidecarState>>,
}

impl Publisher {
    fn set(&mut self, state: SidecarState) {
        self.tx.send_replace(state);
        self.flush();
    }

    /// Answer every waiter if the current state is settled.
    fn flush(&mut self) {
        let state = self.tx.borrow().clone();
        if state.is_settled() {
            for w in self.waiting.drain(..) {
                let _ = w.send(state.clone());
            }
        }
    }
}

/// How a running sidecar's watch ended.
enum Ended {
    /// It exited, or stopped answering and was stopped. A failure.
    Down(String),
    /// Its binary changed on disk and it was stopped to run the new one.
    BinaryChanged,
    /// Stopped on purpose.
    Stop,
}

async fn supervise(
    runtime: Arc<Runtime>,
    tool_dir: PathBuf,
    api_port: u16,
    policy: RestartPolicy,
    tx: watch::Sender<SidecarState>,
    mut cmds: mpsc::UnboundedReceiver<Cmd>,
) {
    let mut out = Publisher { tx, waiting: Vec::new() };
    let name = tool_dir.display().to_string();
    // A previous Nebo that died without stopping its sidecar leaves it running
    // on our socket; retire it before the first launch.
    runtime.cleanup_stale(&tool_dir);

    let mut failures: u32 = 0;
    // Once `Failed`, slow retries run without republishing Starting, so the
    // app's page holds still instead of flapping every minute.
    let mut quiet = false;
    loop {
        if !quiet {
            out.set(SidecarState::Starting);
        }
        let (reason, permanent, immediate) = match runtime.launch(&tool_dir, api_port).await {
            Err(e) => (e.to_string(), e.is_permanent(), false),
            Ok(process) => {
                quiet = false;
                let launched = Arc::new(Launched {
                    pid: process.pid,
                    sock_path: process.sock_path.clone(),
                    app_token: process.app_token.clone(),
                    manifest: process.manifest.clone(),
                });
                out.set(SidecarState::Running(launched));
                let started = Instant::now();
                let (ended, revived) = watch_process(process, &policy, &mut cmds, &mut out).await;
                if started.elapsed() >= policy.healthy_after {
                    failures = 0;
                }
                match ended {
                    Ended::Stop => {
                        out.set(SidecarState::Off);
                        return;
                    }
                    Ended::BinaryChanged => {
                        info!(sidecar = %name, "binary changed on disk — running the new one");
                        failures = 0;
                        continue;
                    }
                    // Found dead by a request: relaunch without the backoff wait.
                    Ended::Down(reason) => (reason, false, revived),
                }
            }
        };

        failures += 1;
        let failed = permanent || failures >= policy.max_failures;
        let delay = if immediate { Duration::ZERO } else if failed { policy.max_delay } else { policy.delay(failures) };
        warn!(
            sidecar = %name,
            attempt = failures,
            permanent,
            retry_in_ms = delay.as_millis() as u64,
            "sidecar is down: {reason}"
        );
        if failed {
            if !quiet {
                out.set(SidecarState::Failed { reason, permanent });
            }
            quiet = true;
        } else {
            out.set(SidecarState::Restarting { attempt: failures, retry_in: delay, reason });
        }

        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            cmd = cmds.recv() => match cmd {
                None | Some(Cmd::Stop) => {
                    out.set(SidecarState::Off);
                    return;
                }
                // "Try again" starts over: a fresh count, published attempts.
                Some(Cmd::Revive(reply)) => {
                    out.waiting.push(reply);
                    failures = 0;
                    quiet = false;
                }
            },
        }
    }
}

/// Watch a running sidecar until it ends: its exit awaited, its socket probed,
/// its binary checked, commands obeyed. Returns how it ended and whether a
/// revive request is what found it down.
async fn watch_process(
    mut process: Process,
    policy: &RestartPolicy,
    cmds: &mut mpsc::UnboundedReceiver<Cmd>,
    out: &mut Publisher,
) -> (Ended, bool) {
    let mut probe = tokio::time::interval(policy.probe_every);
    probe.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    probe.tick().await; // the first tick is immediate; launch already checked the socket
    let mut misses: u32 = 0;
    loop {
        tokio::select! {
            why = process.exited() => return (Ended::Down(why), false),
            _ = probe.tick() => {
                if process.binary_changed() {
                    process.stop().await;
                    return (Ended::BinaryChanged, false);
                }
                if answers(&process).await {
                    misses = 0;
                } else {
                    misses += 1;
                    if misses >= policy.probe_misses {
                        process.stop().await;
                        return (Ended::Down(format!("stopped accepting connections ({misses} probes in a row)")), false);
                    }
                }
            }
            cmd = cmds.recv() => match cmd {
                None | Some(Cmd::Stop) => {
                    process.stop().await;
                    return (Ended::Stop, false);
                }
                Some(Cmd::Revive(reply)) => {
                    out.waiting.push(reply);
                    if !answers(&process).await {
                        process.stop().await;
                        return (Ended::Down("a request could not reach it".into()), true);
                    }
                    out.flush();
                }
            },
        }
    }
}

/// Whether the sidecar accepts a connection on its socket right now.
#[cfg(unix)]
async fn answers(process: &Process) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_secs(2), tokio::net::UnixStream::connect(&process.sock_path)).await,
        Ok(Ok(_))
    )
}

/// Unix sockets only: elsewhere the awaited exit is the liveness signal.
#[cfg(not(unix))]
async fn answers(_process: &Process) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_from_the_first_delay_to_the_cap() {
        let p = RestartPolicy::default();
        let waits: Vec<u64> = (1..=8).map(|n| p.delay(n).as_secs()).collect();
        assert_eq!(waits, vec![1, 2, 4, 8, 16, 32, 60, 60]);
    }
}

/// Real processes: the fixture sidecar, killed, crashed and starved.
#[cfg(all(test, unix))]
mod process_tests {
    use super::*;
    use crate::test_sidecar::{self, TestApp};

    /// Test timings: the same policy, in milliseconds.
    fn quick() -> RestartPolicy {
        RestartPolicy {
            first_delay: Duration::from_millis(50),
            max_delay: Duration::from_millis(400),
            healthy_after: Duration::from_secs(30),
            max_failures: 3,
            probe_every: Duration::from_millis(100),
            probe_misses: 2,
        }
    }

    fn start(app: &TestApp, policy: RestartPolicy) -> Supervisor {
        let data_dir = crate::app_data::data_dir(&app.home, crate::app_data::DataKind::App, &app.id).expect("app id");
        let runtime = Arc::new(Runtime::new(data_dir));
        Supervisor::start(runtime, app.tool_dir.clone(), 0, policy)
    }

    fn pid(state: &SidecarState) -> u32 {
        match state {
            SidecarState::Running(l) => l.pid,
            other => panic!("expected running, got {other:?}"),
        }
    }

    /// Wait until the state satisfies `f`, or fail after `secs`.
    async fn until(sup: &Supervisor, secs: u64, f: impl Fn(&SidecarState) -> bool) -> SidecarState {
        let mut rx = sup.subscribe();
        match tokio::time::timeout(Duration::from_secs(secs), rx.wait_for(|s| f(s))).await {
            Ok(Ok(s)) => s.clone(),
            _ => panic!("state never came: last {:?}", sup.state()),
        }
    }

    #[tokio::test]
    async fn a_killed_sidecar_is_noticed_and_restarted() {
        let app = TestApp::new("killed");
        let sup = start(&app, quick());
        let first = pid(&sup.settled(Duration::from_secs(20)).await);
        test_sidecar::kill(first);
        let again = until(&sup, 10, |s| matches!(s, SidecarState::Running(l) if l.pid != first)).await;
        assert!(tokio::net::UnixStream::connect(app.sock_path()).await.is_ok(), "the new one serves");
        assert_eq!(app.launches(), 2);
        assert!(!test_sidecar::exists(first), "the dead one was reaped, not left a zombie");
        let _ = again;
        sup.shutdown().await;
    }

    #[tokio::test]
    async fn a_live_process_that_stopped_serving_is_restarted_by_the_probe() {
        let app = TestApp::new("deaf");
        app.set_mode("deaf");
        let sup = start(&app, quick());
        let first = pid(&sup.settled(Duration::from_secs(20)).await);
        app.set_mode("serve");
        until(&sup, 10, |s| matches!(s, SidecarState::Running(l) if l.pid != first)).await;
        assert!(tokio::net::UnixStream::connect(app.sock_path()).await.is_ok());
        sup.shutdown().await;
    }

    /// The owner's report: a sidecar process behind a leftover socket file
    /// that refuses connections. With the probe far away, the request that
    /// finds it unreachable revives it through the same restart path.
    #[tokio::test]
    async fn a_request_that_finds_it_unreachable_relaunches_it() {
        let app = TestApp::new("stale");
        app.set_mode("deaf");
        let sup = start(&app, RestartPolicy { probe_every: Duration::from_secs(3600), ..quick() });
        let first = pid(&sup.settled(Duration::from_secs(20)).await);
        assert!(app.sock_path().exists(), "the socket file is there");
        assert!(tokio::net::UnixStream::connect(app.sock_path()).await.is_err(), "and refuses");
        app.set_mode("serve");
        let now = sup.revive(Duration::from_secs(20)).await;
        assert_ne!(pid(&now), first);
        assert!(tokio::net::UnixStream::connect(app.sock_path()).await.is_ok());
        sup.shutdown().await;
    }

    #[tokio::test]
    async fn a_crash_loop_ends_failed_with_its_reason_and_stops_flapping() {
        let app = TestApp::new("crashy");
        app.set_mode("exit 3");
        let sup = start(&app, quick());
        let failed = until(&sup, 20, |s| matches!(s, SidecarState::Failed { .. })).await;
        let SidecarState::Failed { reason, permanent } = failed else { unreachable!() };
        assert!(!permanent);
        assert!(reason.contains("exited with code 3"), "{reason}");
        assert!(reason.contains("fixture: exiting with 3"), "the last output is in the reason: {reason}");
        assert_eq!(app.launches(), 3, "max_failures fast attempts");

        // Out of fast retries: the slow cadence keeps trying without the state
        // moving, so the app's page holds still.
        let mut rx = sup.subscribe();
        rx.borrow_and_update();
        let before = app.launches();
        tokio::time::sleep(Duration::from_millis(1300)).await;
        assert!(!rx.has_changed().unwrap(), "no flapping while failed: {:?}", sup.state());
        let slow = app.launches() - before;
        assert!((1..=4).contains(&slow), "slow retries at max_delay, got {slow}");

        // Fixed, and "Try again": back at once.
        app.set_mode("serve");
        assert!(sup.revive(Duration::from_secs(20)).await.is_running());
        sup.shutdown().await;
    }

    #[tokio::test]
    async fn a_clean_shutdown_is_not_a_crash() {
        let app = TestApp::new("clean");
        let sup = start(&app, quick());
        let first = pid(&sup.settled(Duration::from_secs(20)).await);
        let mut rx = sup.subscribe();
        rx.borrow_and_update();
        sup.shutdown().await;
        assert!(matches!(*rx.borrow(), SidecarState::Off), "off, never restarting or failed");
        assert!(!test_sidecar::exists(first));
        assert!(!app.sock_path().exists(), "its socket is removed");
        assert_eq!(app.launches(), 1);
    }

    #[tokio::test]
    async fn a_missing_program_is_permanent_and_recovers_when_it_appears() {
        let app = TestApp::new("absent");
        app.remove_binary();
        let sup = start(&app, quick());
        let SidecarState::Failed { reason, permanent } = sup.settled(Duration::from_secs(20)).await else {
            panic!("expected failed: {:?}", sup.state());
        };
        assert!(permanent);
        assert!(reason.contains("no binary found"), "{reason}");
        app.install_binary();
        until(&sup, 10, SidecarState::is_running).await;
        sup.shutdown().await;
    }

    #[tokio::test]
    async fn a_rebuilt_binary_replaces_the_running_one() {
        let app = TestApp::new("rebuilt");
        let sup = start(&app, quick());
        let first = pid(&sup.settled(Duration::from_secs(20)).await);
        tokio::time::sleep(Duration::from_millis(20)).await;
        app.install_binary();
        until(&sup, 10, |s| matches!(s, SidecarState::Running(l) if l.pid != first)).await;
        sup.shutdown().await;
    }
}
