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
//! logs again when it clears. Nothing here exits the process: a cloud bot's
//! liveness probe on `/health` (503, or no answer at all) is what restarts
//! it; the desktop app and `make dev` have no supervisor, so there a stall is
//! a loud log and an unhealthy `/health`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

const RUNTIME_TICK: Duration = Duration::from_secs(1);
const DB_PROBE_EVERY: Duration = Duration::from_secs(10);
const WATCH_EVERY: Duration = Duration::from_secs(5);
/// Longer than one pool wait (`db::pool::POOL_WAIT`) plus SQLite's
/// busy_timeout several times over, so a busy moment never reads as a stall.
pub(crate) const STALE_AFTER: Duration = Duration::from_secs(60);

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

    let watchdog = std::thread::Builder::new().name("nebo-watchdog".into()).spawn(move || {
        let mut reported = false;
        loop {
            std::thread::sleep(WATCH_EVERY);
            match live.stall(Instant::now()) {
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
