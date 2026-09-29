//! The run's idle bound: a run whose stream carries no event for
//! [`RUN_IDLE_LIMIT`] while nothing is waited on is ended by the dispatcher
//! as [`STALLED`]. Waiting on the owner is never a stall.

/// The exit label of a run the dispatcher ended for going silent.
pub const STALLED: &str = "stalled";

/// How long a run may go without a single stream event, while none of its
/// calls waits on the owner, before the dispatcher ends it as [`STALLED`].
/// Must outlast the longest thing that is silent while it works: a
/// foreground helper (`delegation::INACTIVITY_LIMIT`) and a shell command at
/// its own timeout. Live 2026-09-02: a run whose turn had ended sat "active"
/// for 38 minutes with nothing bounding it.
pub const RUN_IDLE_LIMIT: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// The owner-facing line for a stalled run. States what was observed and what
/// was done; never guesses at a cause.
pub fn stall_notice() -> String {
    format!(
        "Stopped: nothing happened for {} minutes (no reply text and no tool activity), \
         so the run was ended instead of left hanging. Ask again to continue.",
        RUN_IDLE_LIMIT.as_secs() / 60
    )
}

/// The next event of a run's stream, or `Stalled` once nothing has arrived
/// for [`RUN_IDLE_LIMIT`] while nothing is waited on. The clock counts from
/// the later of `last_event` and the end of the last wait: a question the
/// owner takes an hour to answer, or a day, never ends the run. The ONE idle
/// bound for every loop that drains a run (both chat dispatchers and voice),
/// so a third copy of the select arm cannot drift. Cancel-safe: every arm is.
pub enum Next<T> {
    Event(T),
    Closed,
    Stalled,
}

pub async fn next_event<T>(
    rx: &mut tokio::sync::mpsc::Receiver<T>,
    last_event: tokio::time::Instant,
    waiting: &tools::Waiting,
) -> Next<T> {
    let mut quiet_since = last_event;
    loop {
        let changed = waiting.changed();
        tokio::pin!(changed);
        changed.as_mut().enable();
        let idle = waiting.is_idle();
        tokio::select! {
            ev = rx.recv() => return match ev {
                Some(e) => Next::Event(e),
                None => Next::Closed,
            },
            _ = &mut changed => {
                if waiting.is_idle() {
                    quiet_since = tokio::time::Instant::now();
                }
            }
            _ = tokio::time::sleep_until(quiet_since + RUN_IDLE_LIMIT), if idle => return Next::Stalled,
        }
    }
}

#[cfg(test)]
mod next_event_tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test(start_paused = true)]
    async fn stalls_only_when_nothing_arrives() {
        let waiting = tools::Waiting::default();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<u8>(1);
        tx.send(7).await.expect("channel open");
        assert!(matches!(next_event(&mut rx, tokio::time::Instant::now(), &waiting).await, Next::Event(7)));
        // Sender alive, nothing sent: paused time jumps straight to the limit.
        assert!(matches!(next_event(&mut rx, tokio::time::Instant::now(), &waiting).await, Next::Stalled));
        drop(tx);
        assert!(matches!(next_event(&mut rx, tokio::time::Instant::now(), &waiting).await, Next::Closed));
    }

    /// A question the owner answers a day later is not a stall, and the
    /// idle clock starts over when the wait ends.
    #[tokio::test(start_paused = true)]
    async fn waiting_on_the_owner_is_never_a_stall() {
        let waiting = Arc::new(tools::Waiting::default());
        let (tx, mut rx) = tokio::sync::mpsc::channel::<u8>(1);
        let asked = waiting.enter();
        let answered = {
            let tx = tx.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(24 * 3600)).await;
                drop(asked);
                tokio::time::sleep(RUN_IDLE_LIMIT / 2).await;
                tx.send(1).await.expect("channel open");
            })
        };
        let start = tokio::time::Instant::now();
        assert!(matches!(next_event(&mut rx, start, &waiting).await, Next::Event(1)), "the answer arrived; the run was never ended");
        answered.await.unwrap();
        // With nothing waited on, silence past the limit is a stall again.
        let quiet = tokio::time::Instant::now();
        assert!(matches!(next_event(&mut rx, quiet, &waiting).await, Next::Stalled));
        assert!(tokio::time::Instant::now() - quiet >= RUN_IDLE_LIMIT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A helper's stall must fire before its parent's: otherwise the parent
    /// is ended as stalled while the helper is about to report.
    #[test]
    fn run_idle_limit_outlasts_a_child_stall_and_the_notice_states_the_window() {
        assert!(RUN_IDLE_LIMIT > crate::harness::delegation::INACTIVITY_LIMIT);
        let n = stall_notice();
        assert!(n.contains("15 minutes"), "{n}");
        assert!(!n.contains('\u{2014}'), "no em dash in owner copy");
    }
}
