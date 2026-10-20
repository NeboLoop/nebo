//! Janus's refusal of a request only because work already running holds
//! the account's money: the balance itself covers it. The request was not
//! served or charged. Every caller of Janus reads it here and waits it out
//! with [`ReservedWaits`], in the layer that owns what the wait must not
//! hold: the chat runner (its concurrency slot), `ai::http::send` for a
//! caller with no runner, the embedding gate (its permit) and the voice
//! dial (its connect deadline).

use std::time::Duration;

/// The gateway's code for that refusal, on a 429.
pub const BALANCE_RESERVED: &str = "BALANCE_RESERVED";

/// What the owner reads when the refusal outlasted [`ReservedWaits`]. Never
/// "add funds": the balance covers the work.
pub const BALANCE_BUSY: &str = "Your balance is busy with work that's running. Try again in a moment.";

/// Whether a refusal (its HTTP status and JSON body) is [`BALANCE_RESERVED`]:
/// the code Janus writes at `error.code`, never a match anywhere in the text.
pub fn is_balance_reserved(status: u16, body: &str) -> bool {
    status == 429
        && serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|v| v.pointer("/error/code").and_then(|c| c.as_str()).map(|c| c == BALANCE_RESERVED))
            .unwrap_or(false)
}

/// How many times a refused request is sent again, and the most all the
/// waits between add up to.
pub const RESERVED_RETRIES: usize = 3;
const RESERVED_BUDGET: Duration = Duration::from_secs(30);
/// The wait when the refusal names none.
const RESERVED_WAIT: Duration = Duration::from_secs(2);
/// The least wait, whatever the refusal asks: `Retry-After: 0` would send
/// the refused request again at once, three times over.
const RESERVED_FLOOR: Duration = Duration::from_secs(1);

/// The waits one logical request gets while the balance is held, however
/// many sends it takes: the refusal's `Retry-After` (else [`RESERVED_WAIT`],
/// never under [`RESERVED_FLOOR`]) plus up to a quarter more, so callers
/// refused together do not all come back at once; at most
/// [`RESERVED_RETRIES`] of them within [`RESERVED_BUDGET`]. Made once per
/// request and kept across its other retries, so they never start a fresh
/// budget.
#[derive(Debug, Default)]
pub struct ReservedWaits {
    waited: Duration,
    retries: usize,
}

impl ReservedWaits {
    /// The wait before sending again after a refusal that asked for
    /// `retry_after`, or `None` when the waits are used up.
    pub fn next(&mut self, retry_after: Option<Duration>) -> Option<Duration> {
        if self.retries == RESERVED_RETRIES {
            return None;
        }
        let asked = retry_after.unwrap_or(RESERVED_WAIT).max(RESERVED_FLOOR);
        let wait = asked + jitter(asked / 4);
        if self.waited + wait > RESERVED_BUDGET {
            return None;
        }
        self.retries += 1;
        self.waited += wait;
        Some(wait)
    }
}

/// Up to `max`.
fn jitter(max: Duration) -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    max.mul_f64(f64::from(nanos % 1000) / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_reserved_429_is_reserved() {
        let body = r#"{"error":{"code":"BALANCE_RESERVED","message":"reserved"}}"#;
        assert!(is_balance_reserved(429, body));
        assert!(!is_balance_reserved(503, body));
        assert!(!is_balance_reserved(429, r#"{"error":{"code":"USAGE_LIMIT_EXCEEDED"}}"#));
        // The code, not the words: a message that names it is not the refusal.
        assert!(!is_balance_reserved(
            429,
            r#"{"error":{"code":"USAGE_LIMIT_EXCEEDED","message":"not BALANCE_RESERVED"}}"#
        ));
        assert!(!is_balance_reserved(429, "BALANCE_RESERVED"));
    }

    /// `Retry-After: 0` still waits the floor, so the request is never sent
    /// again at once.
    #[test]
    fn a_zero_wait_still_waits_the_floor() {
        let mut waits = ReservedWaits::default();
        let w = waits.next(Some(Duration::ZERO)).expect("a wait");
        assert!(w >= RESERVED_FLOOR, "{w:?}");
    }

    /// Three waits at most, each the asked wait plus up to a quarter, and
    /// none that would take the total past the budget.
    #[test]
    fn the_waits_are_bounded() {
        let mut waits = ReservedWaits::default();
        for _ in 0..RESERVED_RETRIES {
            let w = waits.next(Some(Duration::from_secs(1))).expect("a wait");
            assert!(w >= Duration::from_secs(1) && w <= Duration::from_millis(1_250), "{w:?}");
        }
        assert_eq!(waits.next(Some(Duration::from_secs(1))), None);

        let mut long = ReservedWaits::default();
        assert_eq!(long.next(Some(Duration::from_secs(60))), None, "a wait past the budget is not taken");
    }
}
