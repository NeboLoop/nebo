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
//!
//! The watchdog counts in the machine's awake time and tells a sleep from a
//! stall (`types::stall`): after a sleep, or a gap in which the whole process
//! got no CPU (`throttled`), the heartbeats start over. Each stall is
//! reported to the platform on the workforce report; a stall in progress is
//! kept in `stall.json`, so one the process does not survive is sent by the
//! next launch as `unrecovered`.

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
    stall_for_test();
    crash_for_test();
}

/// A test's crashing engine: `<data_dir>/TEST_CRASH` makes it abort once
/// the server serves, as a crash would (a supervisor that can't end the
/// process from outside, the house Windows runner's, still sees one). Read
/// once and removed, so the engine started again serves. Only in a
/// relocated Nebo folder (`NEBO_HOME`: a test's), never the owner's.
fn crash_for_test() {
    if !config::data_dir_overridden() {
        return;
    }
    let Ok(path) = config::data_dir().map(|d| d.join("TEST_CRASH")) else { return };
    if std::fs::remove_file(&path).is_ok() {
        tracing::warn!("TEST_CRASH: aborting");
        std::process::abort();
    }
}

/// A test's stalled engine: `<data_dir>/TEST_STALL` holding a number of
/// seconds makes every runtime worker block that long once the server
/// serves, as workers stuck in SQLite would. Read once and removed, so the
/// engine its supervisor starts again serves. Only in a relocated Nebo
/// folder (`NEBO_HOME`: a test's), never the owner's.
fn stall_for_test() {
    if !config::data_dir_overridden() {
        return;
    }
    let Ok(path) = config::data_dir().map(|d| d.join("TEST_STALL")) else { return };
    let Some(secs) = std::fs::read_to_string(&path).ok().and_then(|s| s.trim().parse::<u64>().ok()) else { return };
    let _ = std::fs::remove_file(&path);
    let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
    let workers = rt.metrics().num_workers();
    tracing::warn!(secs, workers, "TEST_STALL: blocking every runtime worker");
    // More than one each: a worker that finds the queue empty steals the next.
    for _ in 0..workers * 2 {
        rt.spawn(async move { std::thread::sleep(Duration::from_secs(secs)) });
    }
}

/// How long the stall has lasted in awake time after one watchdog tick
/// that took `awake` of it: a tick with no stall starts the count over.
fn stalled_for(before: Duration, stalled: bool, awake: Duration) -> Duration {
    if stalled { before + awake } else { Duration::ZERO }
}

/// Seconds since the epoch, for a report's `at`.
fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// The report for a heartbeat stall that began now, with the pool's and the
/// runtime's state at its start.
fn stalled_report(stall: Stall, store: &db::Store, runtime: &tokio::runtime::Handle) -> types::stall::Report {
    let heartbeat = match (stall.runtime_age > STALE_AFTER, stall.db_age > STALE_AFTER) {
        (true, true) => "both",
        (true, false) => "runtime",
        _ => "db",
    };
    let (connections, idle) = store.pool_state();
    let metrics = runtime.metrics();
    let silent = stall.runtime_age.max(stall.db_age);
    types::stall::Report {
        kind: "stalled".into(),
        at: unix_now() - silent.as_secs() as i64,
        heartbeat: Some(heartbeat.into()),
        runtime_silent_secs: Some(stall.runtime_age.as_secs()),
        db_silent_secs: Some(stall.db_age.as_secs()),
        pool_connections: Some(connections),
        pool_idle: Some(idle),
        runtime: Some(types::stall::RuntimeState {
            workers: metrics.num_workers(),
            alive_tasks: metrics.num_alive_tasks(),
            global_queue_depth: metrics.global_queue_depth(),
        }),
        context: types::stall::context(),
        ..Default::default()
    }
}

/// Watchdog ticks between the stamps that say this run is alive (`process::alive`).
const ALIVE_EVERY_TICKS: u32 = 12;

/// Start both heartbeats and the watchdog. Called once, inside the runtime,
/// after the store opens and the port is held.
///
/// Before anything else: how the last run ended. A stall it ended inside
/// (`stall.json`) is sent as `unrecovered`, and a run that did not stop on
/// purpose counts as a restart (`process::begin_run`).
pub(crate) fn start(store: Arc<db::Store>) {
    types::stall::set_context(crate::process::stall_context);
    let data_dir = config::data_dir().ok();
    if let Some(dir) = &data_dir {
        let unrecovered = types::stall::take_unrecovered(dir);
        let stalled_at = unrecovered.as_ref().map(|r| r.at);
        if let Some(report) = unrecovered {
            tracing::error!(kind = %report.kind, at = report.at, "liveness: the last run ended inside a stall");
            // Its context is the stalled run's, kept as written.
            types::stall::record(report);
        }
        crate::process::begin_run(dir, stalled_at);
    }

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
    let runtime = tokio::runtime::Handle::current();
    let watchdog = std::thread::Builder::new().name("nebo-watchdog".into()).spawn(move || {
        // The stall in progress, reported when it clears.
        let mut current: Option<types::stall::Report> = None;
        let mut stalled = Duration::ZERO;
        let mut clocks = types::stall::now();
        let mut ticks: u32 = 0;
        loop {
            std::thread::sleep(WATCH_EVERY);
            ticks = ticks.wrapping_add(1);
            if ticks.is_multiple_of(ALIVE_EVERY_TICKS) {
                crate::process::alive();
            }
            let now = types::stall::now();
            let gap = types::stall::classify(clocks, now, WATCH_EVERY);
            clocks = now;
            if gap.slept() || gap.stalled() {
                if gap.slept() {
                    tracing::info!(slept_secs = gap.slept.as_secs(), "liveness: the machine slept");
                }
                if gap.stalled() {
                    // This thread needs no tokio worker: when it ran late,
                    // the whole process got no CPU (App Nap, EcoQoS, a
                    // machine out of memory).
                    tracing::error!(
                        lag_secs = gap.lag.as_secs(),
                        "liveness: the whole process got no CPU while the machine was awake (throttled)"
                    );
                    types::stall::record(types::stall::Report {
                        kind: "throttled".into(),
                        at: unix_now() - gap.lag.as_secs() as i64,
                        duration_secs: Some(gap.lag.as_secs()),
                        ..Default::default()
                    });
                }
                // The heartbeats aged while the process was asleep or
                // starved, not stuck (on Windows `Instant` counts sleep):
                // they count from now.
                live.begin(Instant::now());
                stalled = Duration::ZERO;
                continue;
            }
            let stall = live.stall(Instant::now());
            stalled = stalled_for(stalled, stall.is_some(), gap.awake);
            if let Some(by) = supervisor.as_deref()
                && stalled >= EXIT_AFTER
                && SERVING.load(Ordering::Relaxed)
            {
                tracing::error!(
                    stalled_secs = stalled.as_secs(),
                    supervisor = by,
                    "liveness: stalled past the limit; exiting so the supervisor starts Nebo again"
                );
                // stall.json was written when the stall began: the next run sends it.
                crate::process::exit_now(crate::process::EXIT_STALL);
            }
            match stall {
                Some(stall) if current.is_none() => {
                    let report = stalled_report(stall, &store, &runtime);
                    tracing::error!(
                        heartbeat = report.heartbeat.as_deref().unwrap_or_default(),
                        runtime_silent_secs = stall.runtime_age.as_secs(),
                        db_silent_secs = stall.db_age.as_secs(),
                        pool_connections = report.pool_connections.unwrap_or_default(),
                        pool_idle = report.pool_idle.unwrap_or_default(),
                        runtime = ?report.runtime,
                        pid = std::process::id(),
                        "liveness: server stalled; /health answers 503. `sample <pid> 5` (macOS) \
                         shows where the workers wait"
                    );
                    if let Some(dir) = &data_dir
                        && let Err(e) = types::stall::write_unrecovered(dir, &report)
                    {
                        tracing::warn!(error = %e, "liveness: stall.json not written");
                    }
                    current = Some(report);
                }
                None => {
                    if let Some(mut report) = current.take() {
                        let lasted = (unix_now() - report.at).max(0) as u64;
                        tracing::warn!(stalled_secs = lasted, "liveness: server answering again after a stall");
                        if let Some(dir) = &data_dir {
                            types::stall::clear_unrecovered(dir);
                        }
                        report.duration_secs = Some(lasted);
                        types::stall::record(report);
                    }
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
