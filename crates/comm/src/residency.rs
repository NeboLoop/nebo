//! Residency, the bot side of the connection: the hub may ask this cloud bot
//! to park (Phase 1B, cold bots — neboloop `internal/residency`).
//!
//! The hub sends a RESIDENCY frame `{"type":"passivate"}` to a lease-aware
//! bot it considers idle. The read loop hands the request to whoever
//! subscribed ([`subscribe`] — the server's residency loop) and answers
//! `{"type":"busy","reason"}` with what it decides, or nothing when the bot
//! goes ahead: it then stops taking deliveries ([`hold_deliveries`]) so
//! everything that arrives from here on waits unacked in its hub mailbox,
//! commits its state, hands its lease back and exits. A process nobody
//! subscribed for (a desktop) declines.
//!
//! Held deliveries are neither handled nor acked, so the hub replays them to
//! whichever process is the bot next — the handoff check brings the bot
//! straight back when any are waiting.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

/// What the hub asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Passivate {
    /// The bot must have been idle this long.
    pub idle_for: Duration,
    /// Its next timer must be at least this far away.
    pub min_next_wake: Duration,
}

/// The bot's decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Not now; the reason goes back to the hub (its logs).
    Busy(String),
    /// Parking: nothing goes back.
    Parking,
}

/// One passivate request and where its decision goes.
pub struct Request {
    pub passivate: Passivate,
    pub answer: oneshot::Sender<Decision>,
}

static HOLD: AtomicBool = AtomicBool::new(false);
static REQUESTS: OnceLock<Mutex<Option<mpsc::Sender<Request>>>> = OnceLock::new();

/// Become the one place passivate requests go. Called once by the server of
/// a bot that can park; a second call takes them over.
pub fn subscribe() -> mpsc::Receiver<Request> {
    let (tx, rx) = mpsc::channel(1);
    *REQUESTS
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = Some(tx);
    rx
}

/// Hand a request to the subscriber. The decision, or `Busy` at once when
/// nothing here can park (no subscriber) or a request is already being
/// decided.
pub fn request(passivate: Passivate) -> oneshot::Receiver<Decision> {
    let (answer, rx) = oneshot::channel();
    let tx = REQUESTS
        .get()
        .and_then(|m| m.lock().unwrap_or_else(|p| p.into_inner()).clone());
    let Some(tx) = tx else {
        let _ = answer.send(Decision::Busy("this bot does not park".into()));
        return rx;
    };
    if let Err(e) = tx.try_send(Request { passivate, answer }) {
        let req = match e {
            mpsc::error::TrySendError::Full(r) | mpsc::error::TrySendError::Closed(r) => r,
        };
        let _ = req.answer.send(Decision::Busy("already deciding".into()));
    }
    rx
}

/// From now on deliveries are neither handled nor acked: they wait in the
/// hub mailbox for the next process. One-way; the process is exiting.
pub fn hold_deliveries() {
    HOLD.store(true, Ordering::Release);
}

/// Whether deliveries are held.
pub fn holding() -> bool {
    HOLD.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASK: Passivate = Passivate {
        idle_for: Duration::from_secs(1800),
        min_next_wake: Duration::from_secs(600),
    };

    // One test: the subscriber is process-wide.
    #[tokio::test]
    async fn requests_reach_the_subscriber_and_nobody_else_parks() {
        // No subscriber: a process that cannot park declines.
        assert_eq!(
            request(ASK).await.unwrap(),
            Decision::Busy("this bot does not park".into())
        );

        let mut rx = subscribe();
        let pending = request(ASK);
        // A second request while the first is undecided is declined, not
        // queued behind it.
        assert_eq!(
            request(ASK).await.unwrap(),
            Decision::Busy("already deciding".into())
        );

        let req = rx.recv().await.unwrap();
        assert_eq!(req.passivate, ASK);
        req.answer.send(Decision::Parking).unwrap();
        assert_eq!(pending.await.unwrap(), Decision::Parking);
        // Holding deliveries is process-wide and would stop every other
        // connection test in this binary: the server's `residency` test
        // binary proves it.
    }
}
