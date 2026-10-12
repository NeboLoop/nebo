//! Sleep or stall: what a long gap between two ticks of a loop was, and the
//! stall reports the platform gets on the workforce report.
//!
//! A loop that ticks every few seconds sometimes sees a much longer gap. Two
//! clocks tell why:
//!
//! | OS | awake (stops while the machine sleeps) | total (counts sleep) |
//! |---|---|---|
//! | macOS | `CLOCK_UPTIME_RAW` | `CLOCK_MONOTONIC_RAW` |
//! | Linux | `CLOCK_MONOTONIC` | `CLOCK_BOOTTIME` |
//! | Windows | `QueryUnbiasedInterruptTime` | `GetTickCount64` |
//!
//! `slept = Δtotal − Δawake` is time the machine was asleep (a closed lid:
//! nothing wrong). `lag = Δawake − expected` is time the machine was awake
//! and this process did not run (a stall). Wall time (`SystemTime`) tells
//! neither: an NTP step looks like sleep, and `Instant` on Windows counts sleep.
//!
//! Reports carry no user content: kinds, durations, counts and the machine's
//! state. One per kind per [`RATE_WINDOW`]; the ones held back are counted in
//! the next one's `suppressed`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// A gap this far past what was expected is a stall, or a sleep.
pub const THRESHOLD: Duration = Duration::from_secs(30);
/// At most one report per kind in this window.
pub const RATE_WINDOW: Duration = Duration::from_secs(600);
/// Reports waiting for the platform's ack, at most.
const PENDING_MAX: usize = 50;

/// Both clocks at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clocks {
    pub awake: Duration,
    pub total: Duration,
}

/// What a gap between two readings was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gap {
    /// Awake time that passed.
    pub awake: Duration,
    /// Time the machine slept.
    pub slept: Duration,
    /// Awake time past what was expected: the process did not run.
    pub lag: Duration,
}

impl Gap {
    /// The process was starved while the machine was awake (ERROR).
    pub fn stalled(&self) -> bool {
        self.lag > THRESHOLD
    }
    /// The machine slept (info): connections made before it are likely dead.
    pub fn slept(&self) -> bool {
        self.slept > THRESHOLD
    }
}

/// The gap from `before` to `after` for a loop that expected `expected`.
pub fn classify(before: Clocks, after: Clocks, expected: Duration) -> Gap {
    let awake = after.awake.saturating_sub(before.awake);
    let total = after.total.saturating_sub(before.total);
    Gap {
        awake,
        slept: total.saturating_sub(awake),
        lag: awake.saturating_sub(expected),
    }
}

/// Both clocks now.
pub fn now() -> Clocks {
    #[cfg(unix)]
    {
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        let (awake, total) = (libc::CLOCK_UPTIME_RAW, libc::CLOCK_MONOTONIC_RAW);
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let (awake, total) = (libc::CLOCK_MONOTONIC, libc::CLOCK_BOOTTIME);
        #[cfg(not(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "linux",
            target_os = "android"
        )))]
        let (awake, total) = (libc::CLOCK_MONOTONIC, libc::CLOCK_MONOTONIC);
        let read = |clock| {
            let mut ts = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            // SAFETY: a valid clock id and a timespec to write into.
            unsafe { libc::clock_gettime(clock, &mut ts) };
            Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
        };
        Clocks {
            awake: read(awake),
            total: read(total),
        }
    }
    #[cfg(windows)]
    {
        let mut hundred_ns: u64 = 0;
        // SAFETY: writes one u64.
        unsafe {
            windows_sys::Win32::System::WindowsProgramming::QueryUnbiasedInterruptTime(
                &mut hundred_ns,
            )
        };
        // SAFETY: no arguments.
        let ms = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() };
        Clocks {
            awake: Duration::from_nanos(hundred_ns.saturating_mul(100)),
            total: Duration::from_millis(ms),
        }
    }
}

// ── Reports ─────────────────────────────────────────────────────────────

/// The runtime's state when a stall began.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeState {
    pub workers: usize,
    pub alive_tasks: usize,
    pub global_queue_depth: usize,
}

/// The machine and the app when a report was made (`set_context`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Context {
    pub app_version: String,
    pub os: String,
    pub os_version: String,
    pub cpus: usize,
    /// One-minute load average; 0 where the OS has none (Windows).
    pub load: f64,
    /// The desktop window: focused, background, hidden, or none.
    pub window_state: String,
    /// The App Nap (macOS) or power-throttling (Windows) opt-out is held.
    pub app_nap_opt_out: bool,
    /// Who restarts this engine: shell, launchd, taskscheduler, systemd; empty for none.
    pub supervisor: String,
}

/// One stall, as the platform stores it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// `stalled` (a heartbeat stopped), `throttled` (the whole process got
    /// no CPU while the machine was awake), `comm_lag` (the NeboAI write loop
    /// ran late while the machine was awake).
    pub kind: String,
    /// When it began, unix seconds.
    pub at: i64,
    /// How long it lasted, when known (it cleared, or it was measured whole).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<u64>,
    /// Which heartbeat stopped: runtime, db, or both (`stalled`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heartbeat: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_silent_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_silent_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool_connections: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool_idle: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeState>,
    /// The process ended inside it (sent by the next launch).
    #[serde(default)]
    pub unrecovered: bool,
    /// Reports of this kind held back by the rate limit before this one.
    #[serde(default)]
    pub suppressed: u32,
    #[serde(flatten)]
    pub context: Context,
}

/// Rate-limited reports waiting to be sent.
#[derive(Default)]
pub struct Recorder {
    last: HashMap<String, Duration>,
    suppressed: HashMap<String, u32>,
    pending: Vec<Report>,
}

impl Recorder {
    /// Queue `report` unless one of its kind went out within [`RATE_WINDOW`]
    /// of `awake` (the awake clock); a report held back is counted on the
    /// next one of its kind. Returns whether it was queued.
    pub fn record(&mut self, mut report: Report, awake: Duration) -> bool {
        if let Some(last) = self.last.get(&report.kind)
            && awake.saturating_sub(*last) < RATE_WINDOW
        {
            *self.suppressed.entry(report.kind).or_default() += 1;
            return false;
        }
        self.last.insert(report.kind.clone(), awake);
        report.suppressed = self.suppressed.remove(&report.kind).unwrap_or(0);
        if self.pending.len() >= PENDING_MAX {
            self.pending.remove(0);
        }
        self.pending.push(report);
        true
    }

    /// The reports not yet acked, oldest first.
    pub fn pending(&self) -> Vec<Report> {
        self.pending.clone()
    }

    /// The platform has the first `n` reports `pending` returned.
    pub fn ack(&mut self, n: usize) {
        self.pending.drain(..n.min(self.pending.len()));
    }
}

static RECORDER: OnceLock<Mutex<Recorder>> = OnceLock::new();
static CONTEXT: OnceLock<fn() -> Context> = OnceLock::new();

fn recorder() -> std::sync::MutexGuard<'static, Recorder> {
    RECORDER
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// What fills each report's [`Context`]; the server sets it once at start.
pub fn set_context(f: fn() -> Context) {
    let _ = CONTEXT.set(f);
}

/// This process's context now (empty before `set_context`).
pub fn context() -> Context {
    CONTEXT.get().map(|f| f()).unwrap_or_default()
}

/// Record a stall for the platform (rate-limited). A report with no context
/// yet gets this process's now; one made earlier (at a stall's start, or by
/// the run before) keeps its own.
pub fn record(mut report: Report) -> bool {
    if report.context.app_version.is_empty() {
        report.context = context();
    }
    recorder().record(report, now().awake)
}

/// The reports waiting for the platform, oldest first.
pub fn pending() -> Vec<Report> {
    recorder().pending()
}

/// The platform has the first `n` of [`pending`].
pub fn ack(n: usize) {
    recorder().ack(n)
}

// ── stall.json: a stall the process may not survive ─────────────────────

/// The file a stall in progress is kept in, under the data directory.
pub const STALL_FILE: &str = "stall.json";

/// Keep `report` in `<dir>/stall.json` while the stall lasts: if the process
/// ends inside it, the next launch sends it (`take_unrecovered`).
pub fn write_unrecovered(dir: &Path, report: &Report) -> std::io::Result<()> {
    let mut report = report.clone();
    report.unrecovered = true;
    let body = serde_json::to_vec(&report).map_err(std::io::Error::other)?;
    let path = dir.join(STALL_FILE);
    let tmp = dir.join(format!("{STALL_FILE}.tmp"));
    std::fs::write(&tmp, body)?;
    std::fs::rename(tmp, path)
}

/// The stall cleared: the file goes.
pub fn clear_unrecovered(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(STALL_FILE));
}

/// The stall the last run ended inside, if any, removing the file.
pub fn take_unrecovered(dir: &Path) -> Option<Report> {
    let path = dir.join(STALL_FILE);
    let body = std::fs::read(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let mut report: Report = serde_json::from_slice(&body).ok()?;
    report.unrecovered = true;
    Some(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK: Duration = Duration::from_secs(5);

    fn at(awake: u64, total: u64) -> Clocks {
        Clocks {
            awake: Duration::from_secs(awake),
            total: Duration::from_secs(total),
        }
    }

    #[test]
    fn an_on_time_tick_is_neither() {
        let g = classify(at(100, 100), at(105, 105), TICK);
        assert!(!g.stalled() && !g.slept());
        assert_eq!(g.lag, Duration::ZERO);
    }

    /// macOS and Linux: the lid closed for an hour moves only the total
    /// clock, so the gap is sleep, never a stall.
    #[test]
    fn a_closed_lid_is_sleep_not_a_stall() {
        let g = classify(at(100, 100), at(105, 3705), TICK);
        assert!(g.slept() && !g.stalled());
        assert_eq!(g.slept, Duration::from_secs(3600));
    }

    /// The App Nap signature: awake the whole time, the process ran 90 s late.
    #[test]
    fn starved_while_awake_is_a_stall() {
        let g = classify(at(100, 100), at(195, 195), TICK);
        assert!(g.stalled() && !g.slept());
        assert_eq!(g.lag, Duration::from_secs(90));
    }

    /// Windows: `Instant` counts sleep there, but QueryUnbiasedInterruptTime
    /// does not, so a wake is sleep with no lag.
    #[test]
    fn a_windows_wake_has_no_lag() {
        // Unbiased interrupt time stood still across a 20-minute sleep;
        // GetTickCount64 moved.
        let g = classify(at(5_000, 5_000), at(5_005, 6_205), TICK);
        assert!(g.slept() && !g.stalled());
    }

    /// Slept and then starved after the wake: both are told.
    #[test]
    fn sleep_then_starvation_is_both() {
        let g = classify(at(0, 0), at(60, 660), TICK);
        assert!(g.slept() && g.stalled());
        assert_eq!(
            (g.slept, g.lag),
            (Duration::from_secs(600), Duration::from_secs(55))
        );
    }

    /// Just under the threshold is neither; a clock read out of order never
    /// underflows.
    #[test]
    fn small_and_backward_gaps_are_neither() {
        assert!(!classify(at(0, 0), at(35, 35), TICK).stalled());
        let g = classify(at(10, 10), at(9, 9), TICK);
        assert_eq!(
            g,
            Gap {
                awake: Duration::ZERO,
                slept: Duration::ZERO,
                lag: Duration::ZERO
            }
        );
    }

    #[test]
    fn the_clocks_move_forward_together() {
        let a = now();
        std::thread::sleep(Duration::from_millis(30));
        let b = now();
        let g = classify(a, b, Duration::from_millis(30));
        assert!(g.awake >= Duration::from_millis(10));
        assert!(!g.slept() && !g.stalled());
    }

    fn report(kind: &str) -> Report {
        Report {
            kind: kind.into(),
            at: 1,
            ..Default::default()
        }
    }

    #[test]
    fn one_per_kind_per_window_with_the_rest_counted() {
        let mut r = Recorder::default();
        let t = Duration::from_secs(1000);
        assert!(r.record(report("stalled"), t));
        assert!(!r.record(report("stalled"), t + Duration::from_secs(60)));
        assert!(!r.record(report("stalled"), t + Duration::from_secs(120)));
        // Another kind is limited on its own.
        assert!(r.record(report("throttled"), t + Duration::from_secs(120)));
        assert!(r.record(report("stalled"), t + RATE_WINDOW));
        let pending = r.pending();
        assert_eq!(
            pending
                .iter()
                .map(|p| (p.kind.as_str(), p.suppressed))
                .collect::<Vec<_>>(),
            vec![("stalled", 0), ("throttled", 0), ("stalled", 2)]
        );
        r.ack(2);
        assert_eq!(r.pending().len(), 1);
        r.ack(5);
        assert!(r.pending().is_empty());
    }

    #[test]
    fn pending_is_bounded() {
        let mut r = Recorder::default();
        for i in 0..(PENDING_MAX + 5) {
            r.record(report(&format!("k{i}")), Duration::ZERO);
        }
        assert_eq!(r.pending().len(), PENDING_MAX);
        assert_eq!(r.pending()[0].kind, "k5");
    }

    #[test]
    fn stall_file_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(take_unrecovered(dir.path()), None);
        let mut stall = report("stalled");
        stall.heartbeat = Some("db".into());
        stall.db_silent_secs = Some(75);
        stall.runtime = Some(RuntimeState {
            workers: 8,
            alive_tasks: 120,
            global_queue_depth: 40,
        });
        stall.context.os_version = "26.5.1".into();
        write_unrecovered(dir.path(), &stall).unwrap();
        let back = take_unrecovered(dir.path()).expect("written");
        assert!(back.unrecovered);
        assert_eq!(
            Report {
                unrecovered: false,
                ..back
            },
            stall
        );
        // Taken once.
        assert_eq!(take_unrecovered(dir.path()), None);
        // Cleared before the process ended: nothing to send.
        write_unrecovered(dir.path(), &stall).unwrap();
        clear_unrecovered(dir.path());
        assert_eq!(take_unrecovered(dir.path()), None);
    }

    #[test]
    fn the_wire_shape_is_flat_camel_case() {
        let mut stall = report("stalled");
        stall.duration_secs = Some(90);
        stall.context.window_state = "hidden".into();
        let v = serde_json::to_value(&stall).unwrap();
        assert_eq!(v["kind"], "stalled");
        assert_eq!(v["durationSecs"], 90);
        assert_eq!(v["windowState"], "hidden");
        assert!(v.get("context").is_none());
        assert!(v.get("heartbeat").is_none());
    }
}
