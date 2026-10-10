//! Liveness: whether this process still serves, as `/health` reports it.
//!
//! On 2026-10-09 the owner's desktop froze: every tokio worker sat in a
//! synchronous SQLite call, the process kept its port, `/health` (which
//! touched nothing) still hung because no worker was free to answer it, and
//! nothing logged a word. Two heartbeats now say what is alive:
//!
//! - the runtime's: a tokio task stamps every [`RUNTIME_TICK`]; it stops only
//!   when no worker is free to run it;
//! - the database's: a dedicated OS thread runs [`db::Store::ping`] every
//!   [`DB_PROBE_EVERY`] and stamps on success; it stops when the pool is held
//!   or SQLite does not answer.
//!
//! Either one older than [`STALE_AFTER`] is a stall: `/health` answers 503
//! whenever a worker can still run it, and a watchdog on its own OS thread
//! (never a tokio worker) logs the stall once, with the pool's state, and
//! logs again when it clears. A cloud bot's liveness probe on `/health` (503,
//! or no answer at all) is what restarts it. A supervised engine (the desktop
//! app's, `process::supervisor`) has no such probe: once it serves, a stall
//! that lasts [`EXIT_AFTER`] of the machine's awake time exits it with
//! `process::EXIT_STALL` from the watchdog's own thread, and its supervisor
//! starts it again. A server nothing restarts only logs.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

const RUNTIME_TICK: Duration = Duration::from_secs(1);
const DB_PROBE_EVERY: Duration = Duration::from_secs(10);
const WATCH_EVERY: Duration = Duration::from_secs(5);
/// Longer than one pool wait (`db::pool::POOL_WAIT`) plus SQLite's
/// busy_timeout several times over, so a busy moment never reads as a stall.
pub(crate) const STALE_AFTER: Duration = Duration::from_secs(60);
/// How long a supervised engine stays stalled, counted in awake time, before
/// the watchdog exits it.
const EXIT_AFTER: Duration = Duration::from_secs(120);

/// Both heartbeats, as milliseconds since `epoch` (monotonic; a laptop's
/// sleep does not age them).
pub(crate) struct Liveness {
    epoch: Instant,
    started: AtomicBool,
    runtime_at: AtomicU64,
    db_at: AtomicU64,
}

/// What has stopped beating, and for how long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stall {
    pub runtime_age: Duration,
    pub db_age: Duration,
}

impl Liveness {
    fn new() -> Self {
        Self {
            epoch: Instant::now(),
            started: AtomicBool::new(false),
            runtime_at: AtomicU64::new(0),
            db_at: AtomicU64::new(0),
        }
    }

    fn millis(&self, at: Instant) -> u64 {
        at.saturating_duration_since(self.epoch).as_millis() as u64
    }

    fn beat(&self, which: &AtomicU64, at: Instant) {
        which.store(self.millis(at), Ordering::Relaxed);
    }

    /// Begin watching: both heartbeats count from now.
    fn begin(&self, at: Instant) {
        self.beat(&self.runtime_at, at);
        self.beat(&self.db_at, at);
        self.started.store(true, Ordering::Relaxed);
    }

    /// The stall at `now`, if either heartbeat is older than [`STALE_AFTER`].
    /// None before watching begins (a router built without `start`).
    pub(crate) fn stall(&self, now: Instant) -> Option<Stall> {
        if !self.started.load(Ordering::Relaxed) {
            return None;
        }
        let now = self.millis(now);
        let age = |at: &AtomicU64| Duration::from_millis(now.saturating_sub(at.load(Ordering::Relaxed)));
        let stall = Stall { runtime_age: age(&self.runtime_at), db_age: age(&self.db_at) };
        (stall.runtime_age > STALE_AFTER || stall.db_age > STALE_AFTER).then_some(stall)
    }
}

/// This process's liveness.
pub(crate) static LIVENESS: LazyLock<Liveness> = LazyLock::new(Liveness::new);

/// Set once the server serves: no stall exits the process while it is still
/// starting (migrations, the first index load).
static SERVING: AtomicBool = AtomicBool::new(false);

/// The server serves: a supervised engine's stall may exit it from now on.
pub(crate) fn serving() {
    SERVING.store(true, Ordering::Relaxed);
}

/// How long the stall has lasted in awake time after one watchdog tick
/// that took `awake` of it: a tick with no stall starts the count over.
fn stalled_for(before: Duration, stalled: bool, awake: Duration) -> Duration {
    if stalled { before + awake } else { Duration::ZERO }
}

/// The machine's awake time: it stops while the machine sleeps, so a laptop
/// with its lid closed for an hour never counts as a stall. (`Instant` is
/// this clock on macOS and Linux, but on Windows it counts sleep too.)
fn awake_now() -> Duration {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    let clock = libc::CLOCK_UPTIME_RAW;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let clock = libc::CLOCK_MONOTONIC;
    #[cfg(unix)]
    {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: a valid clock id and a timespec to write into.
        unsafe { libc::clock_gettime(clock, &mut ts) };
        Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
    }
    #[cfg(windows)]
    {
        let mut hundred_ns: u64 = 0;
        // SAFETY: writes one u64.
        unsafe { windows_sys::Win32::System::WindowsProgramming::QueryUnbiasedInterruptTime(&mut hundred_ns) };
        Duration::from_nanos(hundred_ns.saturating_mul(100))
    }
}

/// Start both heartbeats and the watchdog. Called once, inside the runtime,
/// after the store opens.
pub(crate) fn start(store: Arc<db::Store>) {
    let live = &*LIVENESS;
    live.begin(Instant::now());

    tokio::spawn(async move {
        let mut tick = tokio::time::interval(RUNTIME_TICK);
        loop {
            tick.tick().await;
            live.beat(&live.runtime_at, Instant::now());
        }
    });

    let probe_store = store.clone();
    let probe = std::thread::Builder::new().name("nebo-db-probe".into()).spawn(move || loop {
        match probe_store.ping() {
            Ok(()) => live.beat(&live.db_at, Instant::now()),
            Err(e) => tracing::warn!(error = %e, "liveness: database ping failed"),
        }
        std::thread::sleep(DB_PROBE_EVERY);
    });
    if let Err(e) = probe {
        tracing::error!(error = %e, "liveness: could not start the database probe");
    }

    let supervisor = crate::process::supervisor();
    let watchdog = std::thread::Builder::new().name("nebo-watchdog".into()).spawn(move || {
        let mut reported = false;
        let mut stalled = Duration::ZERO;
        let mut awake = awake_now();
        loop {
            std::thread::sleep(WATCH_EVERY);
            let stall = live.stall(Instant::now());
            let now = awake_now();
            stalled = stalled_for(stalled, stall.is_some(), now.saturating_sub(awake));
            awake = now;
            if let Some(by) = supervisor.as_deref()
                && stalled >= EXIT_AFTER
                && SERVING.load(Ordering::Relaxed)
            {
                tracing::error!(
                    stalled_secs = stalled.as_secs(),
                    supervisor = by,
                    "liveness: stalled past the limit; exiting so the supervisor starts Nebo again"
                );
                std::process::exit(crate::process::EXIT_STALL);
            }
            match stall {
                Some(stall) if !reported => {
                    reported = true;
                    let (connections, idle) = store.pool_state();
                    tracing::error!(
                        runtime_silent_secs = stall.runtime_age.as_secs(),
                        db_silent_secs = stall.db_age.as_secs(),
                        pool_connections = connections,
                        pool_idle = idle,
                        pid = std::process::id(),
                        "liveness: server stalled; /health answers 503. `sample <pid> 5` (macOS) \
                         shows where the workers wait"
                    );
                }
                None if reported => {
                    reported = false;
                    tracing::warn!("liveness: server answering again after a stall");
                }
                _ => {}
            }
        }
    });
    if let Err(e) = watchdog {
        tracing::error!(error = %e, "liveness: could not start the watchdog");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only awake time while stalled counts toward the exit, and any tick
    /// that is not stalled starts the count over.
    #[test]
    fn stall_time_counts_awake_ticks_and_resets() {
        let tick = WATCH_EVERY;
        let mut stalled = Duration::ZERO;
        for _ in 0..(EXIT_AFTER.as_secs() / tick.as_secs() - 1) {
            stalled = stalled_for(stalled, true, tick);
        }
        assert!(stalled < EXIT_AFTER);
        assert!(stalled_for(stalled, true, tick) >= EXIT_AFTER);
        assert_eq!(stalled_for(stalled, false, tick), Duration::ZERO);
        // A tick across a sleep adds only its awake part.
        assert_eq!(stalled_for(Duration::ZERO, true, Duration::from_secs(5)), Duration::from_secs(5));
    }

    #[test]
    fn awake_clock_moves_forward() {
        let a = awake_now();
        std::thread::sleep(Duration::from_millis(20));
        let b = awake_now();
        assert!(b > a && b - a >= Duration::from_millis(10));
    }

    #[test]
    fn not_watching_is_never_a_stall() {
        let live = Liveness::new();
        assert_eq!(live.stall(Instant::now() + STALE_AFTER * 10), None);
    }

    #[test]
    fn fresh_heartbeats_are_healthy() {
        let live = Liveness::new();
        let now = Instant::now();
        live.begin(now);
        assert_eq!(live.stall(now + STALE_AFTER), None);
    }

    /// A database that stops answering is a stall even while the runtime
    /// still ticks, and so is a runtime that stops ticking.
    #[test]
    fn either_stale_heartbeat_is_a_stall() {
        let live = Liveness::new();
        let start = Instant::now();
        live.begin(start);
        let later = start + STALE_AFTER + Duration::from_secs(5);

        live.beat(&live.runtime_at, later);
        let stall = live.stall(later).expect("database silent past STALE_AFTER");
        assert_eq!(stall.runtime_age, Duration::ZERO);
        assert!(stall.db_age > STALE_AFTER);

        live.beat(&live.db_at, later);
        assert_eq!(live.stall(later), None);

        let much_later = later + STALE_AFTER + Duration::from_secs(5);
        live.beat(&live.db_at, much_later);
        let stall = live.stall(much_later).expect("runtime silent past STALE_AFTER");
        assert!(stall.runtime_age > STALE_AFTER);
    }
}
