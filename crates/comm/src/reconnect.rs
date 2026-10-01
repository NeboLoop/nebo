//! When to dial the hub again: the one reconnect policy for the comms
//! connection and the tunnel, in Nebo and in Nebo Link.
//!
//! - The hub closes with 1012 ("drain") when the pod holding the connection
//!   shuts down for a deploy. That is planned: the bot redials after a random
//!   0–3 s, the backoff does not advance, and nothing is shown to anyone.
//! - A close that names another cell (`crate::cell`) is redialed there after
//!   a random 0–500 ms, also without advancing the backoff.
//! - Any other drop, and every failed dial, waits a random time between zero
//!   and `min(30 s, 1 s × 2^n)` (full jitter), so the first redial comes
//!   within a second and thousands of bots dropped together spread out.
//! - A session that stayed up for 10 s was healthy: the next wait starts
//!   from the beginning again.

use std::time::Duration;

/// How long a dial to the hub may take before it counts as failed.
pub const DIAL_TIMEOUT: Duration = Duration::from_secs(5);

/// The close code the hub sends when the pod holding a connection drains
/// (1012 Service Restart, reason "drain").
pub const DRAIN_CLOSE_CODE: u16 = 1012;

/// First wait of the backoff, doubled per attempt.
const BASE: Duration = Duration::from_secs(1);
/// Longest wait of the backoff.
const CAP: Duration = Duration::from_secs(30);
/// A session up this long was healthy: the backoff starts over.
pub(crate) const HEALTHY: Duration = Duration::from_secs(10);
/// Longest wait before redialing after a drain.
const DRAIN_JITTER: Duration = Duration::from_secs(3);

/// How a connection to the hub ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disconnect {
    /// The hub closed with [`DRAIN_CLOSE_CODE`]: its pod is going away for a
    /// deploy, and another one is ready.
    Drain,
    /// The hub closed with [`crate::cell::REDIRECT_CLOSE_CODE`]: the
    /// account lives in another cell, which the next dial goes to.
    Redirect,
    /// Anything else: an error, a reset, a silent hub, or another close.
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
            Disconnect::Dropped => {
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

/// A uniformly random duration in `0..=max`, millisecond resolution.
pub(crate) fn random_up_to(max: Duration) -> Duration {
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

    /// A drain is planned: it waits at most 3 s and never advances the
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
