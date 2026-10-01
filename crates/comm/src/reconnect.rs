//! When to dial the hub again: the one reconnect policy for the comms
//! connection and the tunnel, in Nebo and in Nebo Link.
//!
//! - The hub closes with 1012 ("drain") when the pod holding the connection
//!   shuts down for a deploy, or an edge node leaves. That is planned, and
//!   the sender already spreads its 1012s over its drain window: the bot
//!   redials after a random 0–500 ms, the backoff does not advance, and
//!   nothing is shown to anyone. The redial resolves the hostname again and
//!   spreads across the addresses it finds (`tls::connect_ws`), so a node
//!   that left DNS is not dialed and the others share its connections.
//! - A hard cut — an established connection that ends with no close frame:
//!   a reset, an EOF, a hub gone silent — hits every bot on that node at the
//!   same instant, so the bot spreads it itself: the first redial comes after
//!   a random 0–5 s, and from there the normal backoff below applies.
//! - Every dial tries each address the hub's hostname resolves to, in a
//!   random order, giving each [`tls::CONNECT_TIMEOUT`] to answer; only a
//!   dial that failed on every address counts as failed below.
//! - A close that names another cell (`crate::cell`) is redialed there after
//!   a random 0–500 ms, also without advancing the backoff.
//! - Any other drop, and every failed dial, waits a random time between zero
//!   and `min(30 s, 1 s × 2^n)` (full jitter), so the first redial comes
//!   within a second and thousands of bots dropped together spread out.
//! - A session that stayed up for 10 s was healthy: the next wait starts
//!   from the beginning again.
//! - A CONNECT the hub turns away as busy (a 1013 close, `auth_unavailable`,
//!   a `lease_*` it could not decide) is dialed again here with the same
//!   token, after the same jittered, doubling wait, a few times before the
//!   caller's own backoff takes over ([`dial_through_busy`]). The token is
//!   never refreshed for it.

use std::future::Future;
use std::time::Duration;

use crate::{CommError, ConnectRefusal};

/// How long a connected hub address may take to finish TLS and the upgrade
/// before the dial moves on to the next address (`tls::connect_ws`).
pub const DIAL_TIMEOUT: Duration = tls::HANDSHAKE_TIMEOUT;

/// The close code the hub sends when the pod holding a connection drains
/// (1012 Service Restart, reason "drain").
pub const DRAIN_CLOSE_CODE: u16 = 1012;

/// The close code the hub answers a CONNECT with when it is too busy to
/// take it (1013 Try Again Later).
pub const TRY_AGAIN_LATER_CLOSE_CODE: u16 = 1013;

/// How many times a CONNECT the hub turned away as busy is dialed again by
/// [`dial_through_busy`] before the caller's own backoff takes over.
pub const BUSY_REDIALS: usize = 4;

/// First wait of the backoff, doubled per attempt.
const BASE: Duration = Duration::from_secs(1);
/// Longest wait of the backoff.
const CAP: Duration = Duration::from_secs(30);
/// A session up this long was healthy: the backoff starts over.
pub(crate) const HEALTHY: Duration = Duration::from_secs(10);
/// Longest wait before redialing after a drain.
const DRAIN_JITTER: Duration = Duration::from_millis(500);
/// Longest wait before the first redial after a hard cut.
const CUT_JITTER: Duration = Duration::from_secs(5);

/// How a connection to the hub ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disconnect {
    /// The hub closed with [`DRAIN_CLOSE_CODE`]: its pod is going away for a
    /// deploy, and another one is ready.
    Drain,
    /// The hub closed with [`crate::cell::REDIRECT_CLOSE_CODE`]: the
    /// account lives in another cell, which the next dial goes to.
    Redirect,
    /// An established connection ended with no close frame: a reset, an
    /// EOF, or a hub gone silent.
    Cut,
    /// Anything else: another close code, or a dial that failed.
    Dropped,
}

impl Disconnect {
    /// How a close frame with `code` ended the connection.
    pub fn from_close_code(code: u16) -> Self {
        if code == DRAIN_CLOSE_CODE {
            Disconnect::Drain
        } else {
            Disconnect::Dropped
        }
    }

    /// How an established connection ended: by a close frame with `code`,
    /// or (`None`) with no close frame at all, which is a [`Disconnect::Cut`].
    /// The comms connection and the tunnel both classify through here.
    pub fn from_close(code: Option<u16>) -> Self {
        code.map_or(Disconnect::Cut, Self::from_close_code)
    }
}

/// The wait before each redial of one connection.
#[derive(Debug, Default)]
pub struct Backoff {
    /// Drops and failed dials since the last healthy session.
    attempt: u32,
}

impl Backoff {
    pub fn new() -> Self {
        Self::default()
    }

    /// The wait before redialing a connection that `ended` after being up
    /// for `lived`. A dial that failed is a [`Disconnect::Dropped`] that
    /// lived [`Duration::ZERO`].
    pub fn wait(&mut self, ended: Disconnect, lived: Duration) -> Duration {
        if lived >= HEALTHY {
            self.attempt = 0;
        }
        match ended {
            Disconnect::Drain => random_up_to(DRAIN_JITTER),
            Disconnect::Redirect => crate::cell::jitter(),
            // The first redial after a cut is spread over 0–5 s; a cut that
            // follows a short session counts as a failure like any drop.
            Disconnect::Cut if self.attempt == 0 => {
                self.attempt = 1;
                random_up_to(CUT_JITTER)
            }
            Disconnect::Cut | Disconnect::Dropped => {
                let ceiling = BASE
                    .checked_mul(1u32 << self.attempt.min(16))
                    .unwrap_or(CAP)
                    .min(CAP);
                self.attempt = self.attempt.saturating_add(1);
                random_up_to(ceiling)
            }
        }
    }
}

/// Dial, and while the hub turns the CONNECT away as busy
/// ([`ConnectRefusal::Busy`]) dial again, up to [`BUSY_REDIALS`] times,
/// each after a [`Backoff`] wait (full jitter under a doubling cap), so bots
/// turned away together come back spread out. `sleep` waits out each pause
/// (`tokio::time::sleep`). Quiet: nothing here reaches the owner.
pub async fn dial_through_busy<T, D, DF, S, SF>(mut dial: D, mut sleep: S) -> Result<T, CommError>
where
    D: FnMut() -> DF,
    DF: Future<Output = Result<T, CommError>>,
    S: FnMut(Duration) -> SF,
    SF: Future<Output = ()>,
{
    let mut backoff = Backoff::new();
    let mut result = dial().await;
    for _ in 0..BUSY_REDIALS {
        match &result {
            Err(e) if e.connect_refusal() == ConnectRefusal::Busy => {}
            _ => break,
        }
        sleep(backoff.wait(Disconnect::Dropped, Duration::ZERO)).await;
        result = dial().await;
    }
    result
}

/// A uniformly random duration in `0..=max`, millisecond resolution.
pub fn random_up_to(max: Duration) -> Duration {
    let mut bytes = [0u8; 8];
    if getrandom::getrandom(&mut bytes).is_err() {
        return max / 2;
    }
    let millis = max.as_millis() as u64;
    Duration::from_millis(u64::from_le_bytes(bytes) % (millis + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_1012_is_a_drain() {
        assert_eq!(Disconnect::from_close_code(1012), Disconnect::Drain);
        for code in [1000, 1001, 1006, 1008, 1011, 1013, 4000] {
            assert_eq!(Disconnect::from_close_code(code), Disconnect::Dropped, "code {code}");
        }
    }

    /// Each wait is full jitter under `min(30 s, 1 s × 2^n)`: the first
    /// redial within a second, and never more than 30 s.
    #[test]
    fn drops_wait_full_jitter_under_a_doubling_cap() {
        for _ in 0..200 {
            let mut b = Backoff::new();
            let ceilings = [1, 2, 4, 8, 16, 30, 30, 30, 30, 30];
            for (n, ceiling) in ceilings.iter().enumerate() {
                let w = b.wait(Disconnect::Dropped, Duration::ZERO);
                assert!(w <= Duration::from_secs(*ceiling), "attempt {n}: {w:?} over {ceiling}s");
            }
        }
        // Thousands of attempts never overflow or pass the cap.
        let mut b = Backoff::new();
        for _ in 0..5000 {
            assert!(b.wait(Disconnect::Dropped, Duration::ZERO) <= CAP);
        }
    }

    /// Full jitter spreads the waits: across many bots the same attempt
    /// lands all over its range, not on one instant.
    #[test]
    fn waits_are_spread_across_their_range() {
        let waits: Vec<Duration> = (0..400)
            .map(|_| {
                let mut b = Backoff { attempt: 5 };
                b.wait(Disconnect::Dropped, Duration::ZERO)
            })
            .collect();
        assert!(waits.iter().any(|w| *w < Duration::from_secs(10)));
        assert!(waits.iter().any(|w| *w > Duration::from_secs(20)));
    }

    /// A session that stayed up 10 s was healthy: the next drop is a first
    /// redial again. A shorter one keeps climbing.
    #[test]
    fn a_healthy_session_resets_the_backoff() {
        let mut b = Backoff::new();
        for _ in 0..6 {
            b.wait(Disconnect::Dropped, Duration::ZERO);
        }
        assert_eq!(b.attempt, 6);
        b.wait(Disconnect::Dropped, Duration::from_secs(9));
        assert_eq!(b.attempt, 7, "a 9 s session was short");
        let w = b.wait(Disconnect::Dropped, HEALTHY);
        assert_eq!(b.attempt, 1, "a 10 s session was healthy");
        assert!(w <= Duration::from_secs(1));
    }

    #[test]
    fn a_close_frame_or_its_absence_names_how_the_connection_ended() {
        assert_eq!(Disconnect::from_close(Some(1012)), Disconnect::Drain);
        assert_eq!(Disconnect::from_close(Some(1000)), Disconnect::Dropped);
        assert_eq!(Disconnect::from_close(Some(1006)), Disconnect::Dropped);
        assert_eq!(Disconnect::from_close(None), Disconnect::Cut);
    }

    /// A hard cut of a healthy session: the first redial is spread over
    /// 0–5 s (landing all over that range across many bots), and a redial
    /// that fails after it climbs the normal backoff.
    #[test]
    fn a_cut_spreads_the_first_redial_then_backs_off() {
        let waits: Vec<Duration> = (0..400)
            .map(|_| Backoff::new().wait(Disconnect::Cut, HEALTHY))
            .collect();
        assert!(waits.iter().all(|w| *w <= CUT_JITTER), "{waits:?}");
        assert!(waits.iter().any(|w| *w < Duration::from_secs(1)));
        assert!(waits.iter().any(|w| *w > Duration::from_secs(4)));

        let mut b = Backoff::new();
        b.wait(Disconnect::Cut, HEALTHY);
        assert_eq!(b.attempt, 1);
        for _ in 0..50 {
            let mut again = Backoff { attempt: 1 };
            assert!(again.wait(Disconnect::Dropped, Duration::ZERO) <= Duration::from_secs(2));
        }
        // A cut right after another short session is just another failure.
        let mut b = Backoff { attempt: 4 };
        let w = b.wait(Disconnect::Cut, Duration::from_secs(1));
        assert!(w <= Duration::from_secs(16));
        assert_eq!(b.attempt, 5);
    }

    /// A drain is planned: it waits at most 500 ms and never advances the
    /// backoff, however short the session was.
    #[test]
    fn a_drain_does_not_advance_the_backoff() {
        let mut b = Backoff::new();
        for _ in 0..3 {
            b.wait(Disconnect::Dropped, Duration::ZERO);
        }
        for _ in 0..200 {
            let w = b.wait(Disconnect::Drain, Duration::from_millis(500));
            assert!(w <= DRAIN_JITTER, "{w:?}");
        }
        assert_eq!(b.attempt, 3);
        // A drain after a healthy session still starts the backoff over.
        b.wait(Disconnect::Drain, HEALTHY);
        assert_eq!(b.attempt, 0);
    }
}
