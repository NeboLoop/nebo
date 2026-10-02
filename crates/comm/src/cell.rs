//! Cells: following the hub to the cell that holds the account.
//!
//! The hub runs as cells, each a complete copy of the connection system,
//! and every account lives in exactly one. A connection that reaches a cell
//! the account is not in is refused with a redirect that names the right
//! cell and the address to dial there:
//!
//! - comms `/ws`: an AUTH_FAIL `wrong_cell` frame carrying `cell` and `url`;
//! - the tunnel and `/t/`: an HTTP 421 refusal whose JSON body carries
//!   `{"error":"wrong_cell","cell":…,"url":…}`;
//! - after the upgrade, on either socket: a close with
//!   [`REDIRECT_CLOSE_CODE`] whose reason is the JSON `{"cell":…,"url":…}`.
//!
//! The client redials the named address after a random 0–500 ms, quietly,
//! without advancing its backoff. At most [`MAX_REDIRECTS`] redirects are
//! followed in a row without a session coming up; past that the client goes
//! back to its configured address on the normal backoff
//! ([`crate::reconnect`]).
//!
//! The redirect is a trust boundary: whoever answers the dial picks the next
//! address, and the bot hands that address its token. Only `wss://` hosts in
//! our own domain are followed ([`allowed`]); `ws://` only between loopback
//! addresses, for local development and tests.

use std::time::Duration;

use serde::Deserialize;
use tracing::{info, warn};

/// The close code a cell sends after the upgrade when the account lives in
/// another cell; the reason is the JSON redirect.
pub const REDIRECT_CLOSE_CODE: u16 = 4421;

/// The refusal reason (comms AUTH_FAIL, tunnel and `/t/` 421 body) that
/// carries a redirect.
pub const WRONG_CELL_REASON: &str = "wrong_cell";

/// The HTTP status a cell refuses a dial with when the account lives in
/// another cell.
pub const REDIRECT_STATUS: u16 = 421;

/// Redirects followed in a row without a session coming up. One more is
/// refused, and the client backs off to its configured address.
pub const MAX_REDIRECTS: u32 = 2;

/// Longest wait before dialing the named cell.
const REDIRECT_JITTER: Duration = Duration::from_millis(500);

/// The only domain a redirect may send the bot to.
const HUB_DOMAIN: &str = "neboai.com";

/// Where the hub says the account lives.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Redirect {
    /// The cell's id (for the logs; `url` is what is dialed).
    #[serde(default)]
    pub cell: String,
    /// The full address to dial in that cell.
    #[serde(default)]
    pub url: String,
}

impl Redirect {
    /// A redirect read from a close reason or a refusal body; `None` when it
    /// names no address.
    pub fn from_json(json: &[u8]) -> Option<Self> {
        serde_json::from_slice::<Self>(json)
            .ok()
            .filter(|r| !r.url.is_empty())
    }

    /// The redirect in a close frame, when the code is
    /// [`REDIRECT_CLOSE_CODE`].
    pub fn from_close(code: u16, reason: &str) -> Option<Self> {
        (code == REDIRECT_CLOSE_CODE)
            .then(|| Self::from_json(reason.as_bytes()))
            .flatten()
    }

    /// The redirect in a refused dial: a 421 whose JSON body names an
    /// address.
    pub fn from_dial_error(e: &tokio_tungstenite::tungstenite::Error) -> Option<Self> {
        let tokio_tungstenite::tungstenite::Error::Http(response) = e else {
            return None;
        };
        if response.status().as_u16() != REDIRECT_STATUS {
            return None;
        }
        Self::from_json(response.body().as_deref()?)
    }
}

/// Whether a redirect from a connection to `configured` may send the bot to
/// `target`: `wss://` to `neboai.com` or a host under it, or `ws://` from
/// one loopback address to another. Anything else, including an address
/// carrying credentials, is refused.
pub fn allowed(configured: &str, target: &str) -> bool {
    let Some((scheme, host)) = scheme_and_host(target) else {
        return false;
    };
    match scheme.as_str() {
        "wss" => host == HUB_DOMAIN || host.ends_with(&format!(".{HUB_DOMAIN}")),
        "ws" => {
            is_loopback(&host)
                && scheme_and_host(configured)
                    .is_some_and(|(s, h)| s == "ws" && is_loopback(&h))
        }
        _ => false,
    }
}

fn scheme_and_host(url: &str) -> Option<(String, String)> {
    let uri: tokio_tungstenite::tungstenite::http::Uri = url.parse().ok()?;
    let authority = uri.authority()?;
    if authority.as_str().contains('@') {
        return None;
    }
    Some((
        uri.scheme_str()?.to_ascii_lowercase(),
        authority.host().to_ascii_lowercase(),
    ))
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "[::1]")
}

/// Redirects followed since a session last came up.
#[derive(Debug, Default)]
pub struct Redirects {
    followed: u32,
}

impl Redirects {
    pub fn new() -> Self {
        Self::default()
    }

    /// A session came up: the next redirect is the first again.
    pub fn session_up(&mut self) {
        self.followed = 0;
    }

    /// The address to dial next for `redirect` from a connection to
    /// `configured`, or `None` when it must not be followed: the target is
    /// outside our domain, or [`MAX_REDIRECTS`] were already followed. On
    /// `None` the count starts over, since the caller goes back to
    /// `configured` on its normal backoff.
    pub fn follow(&mut self, configured: &str, redirect: &Redirect) -> Option<String> {
        if !allowed(configured, &redirect.url) {
            warn!(cell = %redirect.cell, url = %::types::redact::redact(&redirect.url), "hub: refusing a redirect outside NeboAI");
            self.followed = 0;
            return None;
        }
        if self.followed >= MAX_REDIRECTS {
            warn!(cell = %redirect.cell, followed = self.followed, "hub: too many redirects in a row; backing off");
            self.followed = 0;
            return None;
        }
        self.followed += 1;
        info!(cell = %redirect.cell, url = %::types::redact::redact(&redirect.url), "hub: account lives in another cell; redialing there");
        Some(redirect.url.clone())
    }
}

/// The wait before dialing the named cell: a random 0–500 ms.
pub fn jitter() -> Duration {
    crate::reconnect::random_up_to(REDIRECT_JITTER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_our_domain_over_tls_is_followed() {
        let cfg = "wss://comms.neboai.com/ws";
        for ok in [
            "wss://neboai.com/ws",
            "wss://comms.neboai.com/ws",
            "wss://cell-2.neboai.com/ws",
            "wss://Cell-2.NeboAI.com:443/tunnel/connect",
        ] {
            assert!(allowed(cfg, ok), "{ok}");
        }
        for bad in [
            "wss://evil.com/ws",
            "wss://neboai.com.evil.com/ws",
            "wss://evilneboai.com/ws",
            "wss://user@cell-2.neboai.com/ws",
            "wss://evil.com@cell-2.neboai.com/ws",
            "ws://cell-2.neboai.com/ws",
            "https://cell-2.neboai.com/ws",
            "ws://127.0.0.1:9/ws",
            "cell-2.neboai.com",
            "",
        ] {
            assert!(!allowed(cfg, bad), "{bad}");
        }
        // Loopback to loopback only (local development and tests).
        assert!(allowed("ws://127.0.0.1:1/ws", "ws://127.0.0.1:2/ws"));
        assert!(allowed("ws://localhost:1/ws", "ws://[::1]:2/ws"));
        assert!(!allowed("ws://127.0.0.1:1/ws", "ws://10.0.0.1:2/ws"));
    }

    #[test]
    fn redirects_are_read_from_close_frames_and_refusal_bodies() {
        let r = Redirect::from_close(4421, r#"{"cell":"2","url":"wss://cell-2.neboai.com/ws"}"#).unwrap();
        assert_eq!(r.cell, "2");
        assert_eq!(r.url, "wss://cell-2.neboai.com/ws");
        assert!(Redirect::from_close(1012, r#"{"cell":"2","url":"wss://cell-2.neboai.com/ws"}"#).is_none());
        assert!(Redirect::from_close(4421, "drain").is_none());
        assert!(Redirect::from_close(4421, r#"{"cell":"2"}"#).is_none());
        let body = br#"{"error":"wrong_cell","cell":"3","url":"wss://cell-3.neboai.com/tunnel/connect"}"#;
        assert_eq!(Redirect::from_json(body).unwrap().cell, "3");
    }

    #[test]
    fn at_most_two_redirects_in_a_row() {
        let cfg = "wss://comms.neboai.com/ws";
        let to = |c: &str| Redirect { cell: c.into(), url: format!("wss://cell-{c}.neboai.com/ws") };
        let mut r = Redirects::new();
        assert!(r.follow(cfg, &to("2")).is_some());
        assert!(r.follow(cfg, &to("3")).is_some());
        assert!(r.follow(cfg, &to("2")).is_none(), "a third redirect in a row is refused");
        // The refusal starts the count over: the next attempt, after the
        // normal backoff, may follow again.
        assert!(r.follow(cfg, &to("2")).is_some());
        r.session_up();
        assert!(r.follow(cfg, &to("3")).is_some());
        assert!(r.follow(cfg, &to("2")).is_some());
    }

    #[test]
    fn a_foreign_host_is_never_followed() {
        let mut r = Redirects::new();
        let foreign = Redirect { cell: "2".into(), url: "wss://evil.example/ws".into() };
        assert!(r.follow("wss://comms.neboai.com/ws", &foreign).is_none());
    }

    #[test]
    fn the_redial_comes_within_half_a_second() {
        for _ in 0..200 {
            assert!(jitter() <= REDIRECT_JITTER);
        }
    }
}
