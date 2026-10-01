//! Releases the hub announces, so a running copy looks for an update at once
//! instead of waiting for its next poll.
//!
//! When a release is published, NeboAI sends every connected copy of that
//! product one small `software_update` message on its `installs` stream:
//! `{"type":"software_update","product":"nebo-link","version":"0.1.3",
//! "urgent":false,"window_s":3600}`. It says only WHEN to look. What is
//! installed still comes from the release feed on the CDN, through the same
//! check, download and signature/checksum verification as a poll.
//!
//! Every copy waits a random moment inside `window_s` before it looks, so a
//! release never brings every copy to the CDN, or back to the hub after a
//! restart, at once. A release that isn't urgent also waits until nothing is
//! running (an agent turn, a run), for at most [`MAX_IDLE_WAIT`]. Each
//! version is acted on once, however many times it is announced.
//!
//! A copy that (re)connects looks too, at a random moment inside
//! [`CONNECTED_JITTER`], unless it looked in the last [`CONNECTED_MIN_GAP`]:
//! that is how a copy that was offline for an announcement still hears of it
//! long before the backstop poll.

use std::collections::HashSet;
use std::time::Duration;

use serde::Deserialize;
use tokio::time::Instant;

/// The `type` of the hub's announcement on the `installs` stream.
pub const ANNOUNCEMENT_TYPE: &str = "software_update";

/// The longest a release that isn't urgent waits for nothing to be running.
pub const MAX_IDLE_WAIT: Duration = Duration::from_secs(6 * 60 * 60);

/// How often a release waiting for idle asks again whether anything runs.
pub const IDLE_RECHECK: Duration = Duration::from_secs(30);

/// The longest spread window a copy accepts from an announcement; a larger
/// one is clamped, so a bad announcement can't postpone an update for days.
pub const MAX_WINDOW: Duration = Duration::from_secs(6 * 60 * 60);

/// The window inside which a copy looks after it (re)connects.
pub const CONNECTED_JITTER: Duration = Duration::from_secs(10 * 60);

/// A (re)connect within this long of the last look doesn't look again: a
/// hub rollout reconnects every copy, and a flapping connection reconnects
/// often.
pub const CONNECTED_MIN_GAP: Duration = Duration::from_secs(15 * 60);

/// A release the hub announced.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Announcement {
    /// The product released: a feed's `binary` ("nebo", "nebo-link").
    pub product: String,
    /// The version released, with or without a leading `v`.
    pub version: String,
    /// Look within the window even while something is running.
    #[serde(default)]
    pub urgent: bool,
    /// The window, in seconds, inside which this copy picks its moment.
    #[serde(default)]
    pub window_s: u64,
}

impl Announcement {
    /// Reads an `installs` stream message: the announcement it carries, or
    /// None for any other message.
    pub fn parse(content: &str) -> Option<Self> {
        #[derive(Deserialize)]
        struct Message {
            #[serde(rename = "type")]
            kind: String,
            #[serde(flatten)]
            announcement: Announcement,
        }
        let message: Message = serde_json::from_str(content).ok()?;
        (message.kind == ANNOUNCEMENT_TYPE).then_some(message.announcement)
    }

    fn window(&self) -> Duration {
        Duration::from_secs(self.window_s).min(MAX_WINDOW)
    }
}

/// What makes the background checker look before its next poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Nudge {
    /// The hub announced a release.
    Announced(Announcement),
    /// The comms connection to the hub came up (again).
    Connected,
}

/// What the checker does when a scheduled look comes due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fire {
    /// Look now.
    Check,
    /// Something is running and the release isn't urgent: ask again later.
    Wait,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Due {
    at: Instant,
    /// Looks whether or not something is running.
    urgent: bool,
    /// Past this, a release that isn't urgent stops waiting for idle.
    idle_until: Instant,
}

/// When nudges make the checker look: the one schedule for announcements
/// and reconnects. `rand` arguments are uniform in `[0, 1)`.
#[derive(Debug)]
pub(crate) struct Schedule {
    product: String,
    current: String,
    /// Versions already acted on (scheduled or looked for).
    handled: HashSet<String>,
    due: Option<Due>,
    last_check: Option<Instant>,
}

impl Schedule {
    pub(crate) fn new(product: &str, current: &str) -> Self {
        Self {
            product: product.to_string(),
            current: crate::normalize_version(current),
            handled: HashSet::new(),
            due: None,
            last_check: None,
        }
    }

    /// When the next look is due, if one is scheduled.
    pub(crate) fn due(&self) -> Option<Instant> {
        self.due.map(|d| d.at)
    }

    /// A look happened (scheduled or a poll).
    pub(crate) fn checked(&mut self, now: Instant) {
        self.last_check = Some(now);
    }

    /// Takes a nudge in. Returns whether it scheduled (or brought forward) a
    /// look.
    pub(crate) fn nudge(&mut self, nudge: Nudge, now: Instant, rand: f64) -> bool {
        match nudge {
            Nudge::Announced(a) => self.announced(a, now, rand),
            Nudge::Connected => {
                let recent = self
                    .last_check
                    .is_some_and(|at| now.saturating_duration_since(at) < CONNECTED_MIN_GAP);
                if recent || self.due.is_some() {
                    return false;
                }
                // Like a poll: it doesn't wait for idle.
                self.schedule(now + CONNECTED_JITTER.mul_f64(rand), true, now)
            }
        }
    }

    fn announced(&mut self, a: Announcement, now: Instant, rand: f64) -> bool {
        let version = crate::normalize_version(&a.version);
        if a.product != self.product
            || self.current == "dev"
            || !crate::is_newer(&version, &self.current)
            || !self.handled.insert(version)
        {
            return false;
        }
        self.schedule(now + a.window().mul_f64(rand), a.urgent, now)
    }

    fn schedule(&mut self, at: Instant, urgent: bool, now: Instant) -> bool {
        let idle_until = at.max(now) + MAX_IDLE_WAIT;
        match &mut self.due {
            Some(due) => {
                let earlier = at < due.at;
                due.at = due.at.min(at);
                due.urgent |= urgent;
                due.idle_until = due.idle_until.min(idle_until);
                earlier || urgent
            }
            None => {
                self.due = Some(Due { at, urgent, idle_until });
                true
            }
        }
    }

    /// The scheduled look came due at `now`; `idle` says nothing is running.
    pub(crate) fn fire(&mut self, now: Instant, idle: bool) -> Fire {
        let Some(due) = self.due.as_mut() else {
            return Fire::Wait;
        };
        if !due.urgent && !idle && now < due.idle_until {
            due.at = now + IDLE_RECHECK;
            return Fire::Wait;
        }
        self.due = None;
        Fire::Check
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn announced(version: &str, urgent: bool, window_s: u64) -> Nudge {
        Nudge::Announced(Announcement {
            product: "nebo-link".into(),
            version: version.into(),
            urgent,
            window_s,
        })
    }

    #[test]
    fn parses_only_the_announcement() {
        let a = Announcement::parse(
            r#"{"type":"software_update","product":"nebo-link","version":"0.1.3","urgent":true,"window_s":120}"#,
        )
        .unwrap();
        assert_eq!(a.product, "nebo-link");
        assert_eq!(a.version, "0.1.3");
        assert!(a.urgent);
        assert_eq!(a.window_s, 120);
        let bare = Announcement::parse(r#"{"type":"software_update","product":"nebo","version":"v1.0.0"}"#).unwrap();
        assert!(!bare.urgent);
        assert_eq!(bare.window_s, 0);
        assert!(Announcement::parse(r#"{"type":"tool_installed","tool_id":"x"}"#).is_none());
        assert!(Announcement::parse(r#"{"type":"software_update"}"#).is_none());
        assert!(Announcement::parse("not json").is_none());
    }

    #[test]
    fn the_moment_is_inside_the_window() {
        let now = Instant::now();
        for rand in [0.0, 0.25, 0.5, 0.999] {
            let mut s = Schedule::new("nebo-link", "0.1.2");
            assert!(s.nudge(announced("0.1.3", false, 3600), now, rand));
            let due = s.due().unwrap();
            assert!(due >= now && due < now + Duration::from_secs(3600), "{rand}: {due:?}");
            assert_eq!(due, now + Duration::from_secs(3600).mul_f64(rand));
        }
    }

    #[test]
    fn a_huge_window_is_clamped() {
        let now = Instant::now();
        let mut s = Schedule::new("nebo-link", "0.1.2");
        s.nudge(announced("0.1.3", false, 30 * 24 * 3600), now, 0.999);
        assert!(s.due().unwrap() < now + MAX_WINDOW);
    }

    #[test]
    fn each_version_is_acted_on_once() {
        let now = Instant::now();
        let mut s = Schedule::new("nebo-link", "0.1.2");
        assert!(s.nudge(announced("0.1.3", false, 60), now, 0.5));
        assert_eq!(s.fire(s.due().unwrap(), true), Fire::Check);
        assert!(!s.nudge(announced("v0.1.3", false, 60), now, 0.5), "announced again");
        assert!(s.due().is_none());
        assert!(s.nudge(announced("0.1.4", false, 60), now, 0.5), "a newer one is new");
    }

    #[test]
    fn other_products_and_older_versions_are_ignored() {
        let now = Instant::now();
        let mut s = Schedule::new("nebo-link", "0.1.2");
        let mut other = Announcement {
            product: "nebo".into(),
            version: "9.0.0".into(),
            urgent: true,
            window_s: 0,
        };
        assert!(!s.nudge(Nudge::Announced(other.clone()), now, 0.0));
        other.product = "nebo-link".into();
        other.version = "0.1.2".into();
        assert!(!s.nudge(Nudge::Announced(other.clone()), now, 0.0), "same version");
        other.version = "0.1.1".into();
        assert!(!s.nudge(Nudge::Announced(other), now, 0.0), "older");
        assert!(s.due().is_none());
        let mut dev = Schedule::new("nebo-link", "dev");
        assert!(!dev.nudge(announced("9.0.0", true, 0), now, 0.0), "a dev build never updates");
    }

    #[test]
    fn urgent_looks_at_its_moment_even_while_running() {
        let now = Instant::now();
        let mut s = Schedule::new("nebo-link", "0.1.2");
        s.nudge(announced("0.1.3", true, 120), now, 0.1);
        let due = s.due().unwrap();
        assert!(due < now + Duration::from_secs(120));
        assert_eq!(s.fire(due, false), Fire::Check);
        assert!(s.due().is_none());
    }

    #[test]
    fn normal_waits_while_running_then_looks_once_idle() {
        let now = Instant::now();
        let mut s = Schedule::new("nebo-link", "0.1.2");
        s.nudge(announced("0.1.3", false, 600), now, 0.5);
        let due = s.due().unwrap();
        assert_eq!(s.fire(due, false), Fire::Wait);
        assert_eq!(s.due(), Some(due + IDLE_RECHECK), "asks again shortly");
        assert_eq!(s.fire(due + IDLE_RECHECK, false), Fire::Wait);
        assert_eq!(s.fire(due + IDLE_RECHECK * 2, true), Fire::Check);
        assert!(s.due().is_none());
    }

    #[test]
    fn normal_stops_waiting_for_idle_after_the_bound() {
        let now = Instant::now();
        let mut s = Schedule::new("nebo-link", "0.1.2");
        s.nudge(announced("0.1.3", false, 0), now, 0.0);
        let mut at = s.due().unwrap();
        let mut waits = 0;
        while s.fire(at, false) == Fire::Wait {
            waits += 1;
            at = s.due().unwrap();
            assert!(at <= now + MAX_IDLE_WAIT + IDLE_RECHECK, "never waits past the bound");
        }
        assert!(at >= now + MAX_IDLE_WAIT);
        assert_eq!(waits as u64, MAX_IDLE_WAIT.as_secs() / IDLE_RECHECK.as_secs());
    }

    #[test]
    fn urgent_brings_a_waiting_release_forward() {
        let now = Instant::now();
        let mut s = Schedule::new("nebo-link", "0.1.2");
        s.nudge(announced("0.1.3", false, 6 * 3600), now, 0.9);
        assert!(s.nudge(announced("0.1.4", true, 60), now, 0.5));
        let due = s.due().unwrap();
        assert_eq!(due, now + Duration::from_secs(30));
        assert_eq!(s.fire(due, false), Fire::Check, "urgent now, running or not");
    }

    #[test]
    fn connecting_looks_soon_unless_it_just_looked() {
        let now = Instant::now();
        let mut s = Schedule::new("nebo-link", "0.1.2");
        assert!(s.nudge(Nudge::Connected, now, 0.5));
        let due = s.due().unwrap();
        assert_eq!(due, now + CONNECTED_JITTER.mul_f64(0.5));
        assert_eq!(s.fire(due, false), Fire::Check, "like a poll, it doesn't wait for idle");
        s.checked(due);
        assert!(!s.nudge(Nudge::Connected, due + Duration::from_secs(60), 0.5), "looked a minute ago");
        assert!(s.nudge(Nudge::Connected, due + CONNECTED_MIN_GAP, 0.5));
    }
}
