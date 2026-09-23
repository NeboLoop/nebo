//! Residency, the server side: whether this cloud bot may park (Phase 1B,
//! cold bots — neboloop `internal/residency`).
//!
//! The hub parks a bot it believes idle: it asks over the gateway
//! (`comm::residency`), and this loop decides with its own, fresher view.
//! Idle is the predicate of plan 3.8 — no run in the [`RunRegistry`], no
//! workflow turn queued or running or owed a resume, nothing in the engine
//! deliverable now, no session wake being delivered — held for at least the
//! hub's threshold; parking is also refused while a channel bridge, a watch
//! binding or folder watcher, or a browser holds this bot (the hub cannot
//! hold those for it), and while its next timer is closer than the hub's
//! minimum.
//!
//! Parking is the ONE graceful drain (`lib.rs`), started by this instead of
//! a signal: deliveries are held first so whatever arrives from then on waits
//! in the hub mailbox, then the drain runs, arms the timers, commits BotState
//! whether or not it changed (the commit carries `next_wake` and the signals
//! the hub's handoff check reads), hands the lease back and exits 0.
//!
//! The same signals ride every BotState commit ([`signals`]), which is what
//! the hub's eligibility test reads.
//!
//! [`RunRegistry`]: crate::run_registry::RunRegistry

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use tokio::sync::Notify;
use tracing::{info, warn};

use crate::state::AppState;

/// When this process last started or finished a run (unix seconds). Written
/// by the run registry; the latest of it, the last workflow turn's end and
/// the process start is when the bot's last work ended.
static LAST_RUN_CHANGE: AtomicI64 = AtomicI64::new(0);
static STARTED: AtomicI64 = AtomicI64::new(0);
static PARKING: AtomicBool = AtomicBool::new(false);

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A run started or finished.
pub fn touch() {
    LAST_RUN_CHANGE.fetch_max(now(), Ordering::Relaxed);
}

/// Whether this process is parking: the drain it started commits even an
/// unchanged state, so the hub has the generation it asked for.
pub fn parking() -> bool {
    PARKING.load(Ordering::Acquire)
}

/// Whether the bot is idle, and since when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Idle {
    /// Why it is not idle; `None` when it is.
    pub busy: Option<String>,
    /// When its last work ended (unix seconds).
    pub since: i64,
}

/// The idle predicate.
pub async fn idle(state: &AppState) -> Idle {
    let t = now();
    let since = [
        LAST_RUN_CHANGE.load(Ordering::Relaxed),
        STARTED.load(Ordering::Relaxed),
        state
            .store
            .engine_last_run_end()
            .ok()
            .flatten()
            .unwrap_or(0),
    ]
    .into_iter()
    .max()
    .unwrap_or(0);
    let runs = state.run_registry.list_all().await.len();
    let busy = if runs > 0 {
        Some(format!("{runs} run(s) in flight"))
    } else {
        match state.store.engine_busy(t) {
            Ok(Some(why)) => Some(why),
            Ok(None) => match crate::wake::in_flight() {
                0 => None,
                n => Some(format!("{n} session wake(s) being delivered")),
            },
            Err(e) => Some(format!("engine unreadable: {e}")),
        }
    };
    Idle { busy, since }
}

/// What holds this bot that the hub cannot hold for it.
async fn pinned(state: &AppState) -> Option<String> {
    let bridges = state.channel_bridges.read().await.len();
    if bridges > 0 {
        return Some(format!("{bridges} channel bridge(s) running"));
    }
    let watchers = agent::running_watchers();
    if watchers > 0 {
        return Some(format!("{watchers} watcher(s) running"));
    }
    let data_dir = config::data_dir().ok()?;
    if crate::backup_ship::chromium_running(&data_dir) {
        return Some("the browser is running".into());
    }
    None
}

/// The residency signals every BotState commit carries (the hub's
/// eligibility test reads them from the latest generation).
pub async fn signals(state: &AppState) -> serde_json::Value {
    let idle = idle(state).await;
    let data_dir = config::data_dir().ok();
    serde_json::json!({
        "idle": idle.busy.is_none(),
        "idle_since": idle.since,
        "channel_bridges": state.channel_bridges.read().await.len(),
        "watch_bindings": agent::running_watchers(),
        "chromium_running": data_dir.as_deref().is_some_and(crate::backup_ship::chromium_running),
    })
}

/// Decide one passivate request. `Parking` has already held deliveries and
/// set [`parking`]; the caller starts the drain.
async fn decide(state: &AppState, ask: comm::residency::Passivate) -> comm::residency::Decision {
    use comm::residency::Decision;
    if parking() {
        return Decision::Parking;
    }
    let idle = idle(state).await;
    if let Some(why) = idle.busy {
        return Decision::Busy(why);
    }
    let quiet = now() - idle.since;
    if quiet < ask.idle_for.as_secs() as i64 {
        return Decision::Busy(format!("last work ended {quiet}s ago"));
    }
    if let Some(why) = pinned(state).await {
        return Decision::Busy(why);
    }
    // The next timer is measured after arming, exactly as the commit's
    // next_wake will be.
    crate::engine::arm_timers(state).await;
    match state.store.engine_next_timer_due() {
        Ok(Some(due)) if due - now() < ask.min_next_wake.as_secs() as i64 => {
            return Decision::Busy(format!("next timer in {}s", due - now()));
        }
        Err(e) => return Decision::Busy(format!("timers unreadable: {e}")),
        _ => {}
    }
    comm::residency::hold_deliveries();
    PARKING.store(true, Ordering::Release);
    Decision::Parking
}

/// Take passivate requests for this cloud bot. `drain` is notified when the
/// bot parks: it is the graceful drain's trigger.
pub fn spawn(state: AppState, drain: Arc<Notify>) {
    STARTED.store(now(), Ordering::Relaxed);
    let mut requests = comm::residency::subscribe();
    tokio::spawn(async move {
        while let Some(req) = requests.recv().await {
            let decision = decide(&state, req.passivate).await;
            let parks = decision == comm::residency::Decision::Parking;
            match &decision {
                comm::residency::Decision::Parking => {
                    info!(
                        "residency: parking — deliveries held, draining to commit and hand the lease back"
                    )
                }
                comm::residency::Decision::Busy(why) => {
                    info!(reason = %why, "residency: not parking")
                }
            }
            if req.answer.send(decision).is_err() {
                warn!("residency: the connection that asked is gone");
            }
            // A repeated ask while parking is answered "parking" again
            // (decide), never "busy": the hub must not read a refusal.
            if parks {
                drain.notify_one();
            }
        }
    });
}
