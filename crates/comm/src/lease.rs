//! BotLease, the bot side: at most one running process acts as a bot.
//!
//! A bot's NeboAI token never expires, and until leases the hub let the last
//! process to dial in take the bot. Twice on 2026-09-21 a scratch server
//! holding the owner's token took over his bot. Now the hub keeps one lease
//! per bot (neboloop `internal/botlease`): the process that holds it is named
//! by the instance id minted here once per process start, and every
//! acquisition by a different process gets a higher epoch.
//!
//! This process acquires on its gateway CONNECT (`neboai.rs`) and renews on
//! its gateway ping every [`RENEW_EVERY`]; the hub answers `lease_ok` or
//! `lease_lost`. Lease age is measured on the monotonic clock from when the
//! renewal that was confirmed was SENT — never from when its answer arrived —
//! so this process always believes its lease ends no later than the hub does.
//! [`RENEW_EVERY`] short of the lease's TTL without a confirmation, the lease
//! is `Uncertain`: the hub cannot give the bot to another process until the
//! full TTL has passed, so this process is frozen before any successor can act.
//!
//! Frozen (`Uncertain` or `Lost`) is enforced only where freezing is on — a
//! cloud bot (see [`Lease::set_fenced`]). Nothing with a side effect runs
//! while frozen: tool calls that change something, outbound sends, workflow
//! advancement, timers. Reads keep working. A desktop still gets the hub's
//! refusal of a second process, without freezing itself, so a laptop with no
//! internet keeps doing local work.
//!
//! The state is written by the connection's own tasks (CONNECT, the read
//! loop) and read lock-free at every gate.

use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

/// How often the bot renews: its gateway ping cadence.
pub const RENEW_EVERY: Duration = Duration::from_secs(15);

/// The lease TTL assumed until the hub states its own (`leaseTtlSecs`).
const DEFAULT_TTL: Duration = Duration::from_secs(60);

/// What a gated operation answers while this bot is frozen. Model-facing
/// (tool results) and owner-facing (logs, ledger), so it states what
/// happened and promises nothing the code does not do.
pub const PAUSED: &str = "Paused: this bot lost its connection to NeboAI, so this was not done and nothing was sent or changed. It can be done again once the connection returns. Do not report it as done.";

/// Where this process stands with the bot's lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseState {
    /// This process has no NeboAI identity to hold a lease for.
    Unclaimed,
    /// The hub does not issue leases (it predates them); nothing to fence.
    Unleased,
    /// This process holds the lease, confirmed by the hub at `confirmed_at`
    /// (the send time of the confirmed request).
    Held { epoch: u64, confirmed_at: Instant },
    /// Not confirmed recently enough to act on: before the first grant, or
    /// [`RENEW_EVERY`] short of the TTL since the last confirmation.
    Uncertain,
    /// The hub gave the bot to another process, or refused it to this one.
    Lost,
}

const UNCLAIMED: u8 = 0;
const CLAIMED: u8 = 1;
const HELD: u8 = 2;
const LOST: u8 = 3;
const UNLEASED: u8 = 4;

/// The lease of this process. There is exactly one per process —
/// [`process`] — because the instance id IS the process.
pub struct Lease {
    instance_id: String,
    /// Whether Uncertain/Lost freeze this process (cloud bots).
    fenced: AtomicBool,
    status: AtomicU8,
    epoch: AtomicU64,
    /// Monotonic milliseconds since [`BASE`] of the latest confirmation.
    confirmed_ms: AtomicU64,
    ttl_ms: AtomicU64,
    /// Set by the graceful drain: this process is shutting down and its
    /// connection hands the lease back as it closes.
    releasing: AtomicBool,
    /// Signalled when the hub answers a claim: a grant (or an unleased hub)
    /// lets hub work start; a refusal sends the process back to asking.
    changed: Notify,
}

/// Origin of the monotonic stamps this process puts on the wire.
static BASE: LazyLock<Instant> = LazyLock::new(Instant::now);

static PROCESS: LazyLock<Lease> = LazyLock::new(Lease::new);

/// This process's lease.
pub fn process() -> &'static Lease {
    &PROCESS
}

impl Default for Lease {
    fn default() -> Self {
        Self::new()
    }
}

impl Lease {
    /// A lease with a fresh instance id, not yet claimed, not fenced.
    pub fn new() -> Self {
        // Stamps are measured from BASE; fix it before any instant this
        // lease will be asked to stamp can be taken.
        LazyLock::force(&BASE);
        Self {
            instance_id: uuid::Uuid::new_v4().to_string(),
            fenced: AtomicBool::new(false),
            status: AtomicU8::new(UNCLAIMED),
            epoch: AtomicU64::new(0),
            confirmed_ms: AtomicU64::new(0),
            ttl_ms: AtomicU64::new(DEFAULT_TTL.as_millis() as u64),
            releasing: AtomicBool::new(false),
            changed: Notify::new(),
        }
    }

    /// The id the hub knows this process by.
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Turn freezing on (cloud bots) or off (desktops).
    pub fn set_fenced(&self, fenced: bool) {
        self.fenced.store(fenced, Ordering::Release);
    }

    /// This process is about to act as its NeboAI bot: until the hub grants
    /// the lease it is `Uncertain`. Idempotent; never un-does a grant.
    pub fn claim(&self) {
        let _ =
            self.status
                .compare_exchange(UNCLAIMED, CLAIMED, Ordering::AcqRel, Ordering::Acquire);
    }

    /// The hub granted the lease at `epoch` (AUTH_OK) to a CONNECT sent at
    /// `sent_at`. Epoch 0 means the hub issues no leases.
    pub fn granted(&self, epoch: u64, ttl: Duration, sent_at: Instant) {
        if epoch == 0 {
            self.status.store(UNLEASED, Ordering::Release);
        } else {
            self.ttl_ms.store(ttl.as_millis() as u64, Ordering::Release);
            self.confirmed_ms
                .store(self.stamp(sent_at), Ordering::Release);
            self.epoch.store(epoch, Ordering::Release);
            self.status.store(HELD, Ordering::Release);
        }
        self.changed.notify_waiters();
    }

    /// The hub renewed `epoch` for a request sent at `sent_at`. Ignored
    /// unless it renews the lease this process holds; never moves the
    /// confirmation backwards (answers can arrive out of order).
    pub fn renewed(&self, epoch: u64, sent_at: Instant) {
        if self.status.load(Ordering::Acquire) != HELD
            || self.epoch.load(Ordering::Acquire) != epoch
        {
            return;
        }
        self.confirmed_ms
            .fetch_max(self.stamp(sent_at), Ordering::AcqRel);
    }

    /// The hub says another process holds the bot.
    pub fn lost(&self) {
        self.status.store(LOST, Ordering::Release);
        self.changed.notify_waiters();
    }

    /// The epoch this process last held, 0 if none.
    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }

    /// Where this process stands now.
    pub fn state(&self) -> LeaseState {
        self.state_at(Instant::now())
    }

    fn state_at(&self, now: Instant) -> LeaseState {
        match self.status.load(Ordering::Acquire) {
            UNCLAIMED => LeaseState::Unclaimed,
            UNLEASED => LeaseState::Unleased,
            LOST => LeaseState::Lost,
            HELD => {
                let epoch = self.epoch.load(Ordering::Acquire);
                let confirmed_at = self.instant_of(self.confirmed_ms.load(Ordering::Acquire));
                let ttl = Duration::from_millis(self.ttl_ms.load(Ordering::Acquire));
                if now.saturating_duration_since(confirmed_at) >= ttl.saturating_sub(RENEW_EVERY) {
                    LeaseState::Uncertain
                } else {
                    LeaseState::Held {
                        epoch,
                        confirmed_at,
                    }
                }
            }
            _ => LeaseState::Uncertain,
        }
    }

    /// Whether side effects must not run now: freezing is on and the lease
    /// is `Uncertain` or `Lost`.
    pub fn frozen(&self) -> bool {
        self.frozen_at(Instant::now())
    }

    fn frozen_at(&self, now: Instant) -> bool {
        self.fenced.load(Ordering::Acquire)
            && matches!(self.state_at(now), LeaseState::Uncertain | LeaseState::Lost)
    }

    /// The epoch to renew: `Some` while this process holds the lease, however
    /// long since it was last confirmed.
    pub fn held_epoch(&self) -> Option<u64> {
        (self.status.load(Ordering::Acquire) == HELD).then(|| self.epoch.load(Ordering::Acquire))
    }

    /// The graceful drain is done with the bot: when this process's gateway
    /// connection closes, it hands the lease back (a CLOSE frame with
    /// `{"releaseLease":true}`) so the next process need not wait out the TTL.
    pub fn release(&self) {
        self.releasing.store(true, Ordering::Release);
    }

    /// The epoch to hand back as the connection closes: `Some` once
    /// [`Lease::release`] was called and while this process holds the lease.
    pub fn release_epoch(&self) -> Option<u64> {
        if !self.releasing.load(Ordering::Acquire) {
            return None;
        }
        self.held_epoch()
    }

    /// Whether the hub told this process it may not hold the bot.
    pub fn is_lost(&self) -> bool {
        self.status.load(Ordering::Acquire) == LOST
    }

    /// Resolves once this process may present itself to the hub's other
    /// doors (the tunnel): it holds a lease, or the hub issues none.
    pub async fn granted_or_unleased(&self) {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if matches!(self.status.load(Ordering::Acquire), HELD | UNLEASED) {
                return;
            }
            notified.await;
        }
    }

    /// Resolves once the hub has refused this process the bot (`Lost`).
    pub async fn until_lost(&self) {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_lost() {
                return;
            }
            notified.await;
        }
    }

    /// Resolves once this process is not frozen (immediately where freezing
    /// is off). Checked every second: a lease also thaws by a renewal, which
    /// signals nobody.
    pub async fn until_unfrozen(&self) {
        while self.frozen() {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// Monotonic stamp (ms since process start) for `at`, as sent on the
    /// wire in a renewal and echoed back by the hub.
    pub fn stamp(&self, at: Instant) -> u64 {
        at.saturating_duration_since(*BASE).as_millis() as u64
    }

    /// The instant a stamp names.
    pub fn instant_of(&self, stamp: u64) -> Instant {
        *BASE + Duration::from_millis(stamp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fenced() -> Lease {
        let l = Lease::new();
        l.set_fenced(true);
        l
    }

    #[test]
    fn instance_ids_are_per_lease() {
        assert_ne!(Lease::new().instance_id(), Lease::new().instance_id());
        assert!(uuid::Uuid::parse_str(process().instance_id()).is_ok());
    }

    /// A process with no NeboAI identity has nothing to lose, so nothing
    /// freezes; once it claims, it is frozen until the hub grants.
    #[test]
    fn frozen_from_claim_until_the_first_grant() {
        let l = fenced();
        assert_eq!(l.state(), LeaseState::Unclaimed);
        assert!(!l.frozen());
        l.claim();
        assert_eq!(l.state(), LeaseState::Uncertain);
        assert!(
            l.frozen(),
            "a claimed bot must not act before the hub grants it"
        );
        let now = Instant::now();
        l.granted(3, Duration::from_secs(60), now);
        assert_eq!(
            l.state(),
            LeaseState::Held {
                epoch: 3,
                confirmed_at: l.instant_of(l.stamp(now))
            }
        );
        assert!(!l.frozen());
        l.claim();
        assert!(
            matches!(l.state(), LeaseState::Held { .. }),
            "claim never undoes a grant"
        );
    }

    /// TTL 60 s, renew every 15 s: frozen at 45 s after the confirmed SEND.
    #[test]
    fn uncertain_at_ttl_minus_renew_interval() {
        let l = fenced();
        let sent = Instant::now();
        l.granted(1, Duration::from_secs(60), sent);
        assert!(!l.frozen_at(sent + Duration::from_millis(44_999)));
        assert!(l.frozen_at(sent + Duration::from_secs(45)));
        assert_eq!(
            l.state_at(sent + Duration::from_secs(45)),
            LeaseState::Uncertain
        );

        // A renewal of the held epoch moves the window; one for another
        // epoch, or an older one arriving late, does not.
        l.renewed(1, sent + Duration::from_secs(30));
        assert!(!l.frozen_at(sent + Duration::from_secs(74)));
        assert!(l.frozen_at(sent + Duration::from_secs(75)));
        l.renewed(1, sent + Duration::from_secs(15));
        assert!(
            l.frozen_at(sent + Duration::from_secs(75)),
            "a late answer must not move the confirmation back"
        );
        l.renewed(2, sent + Duration::from_secs(60));
        assert!(
            l.frozen_at(sent + Duration::from_secs(75)),
            "a renewal of another epoch is not ours"
        );
    }

    /// The hub's own TTL, when it states one, sets the window.
    #[test]
    fn the_hubs_ttl_sets_the_window() {
        let l = fenced();
        let sent = Instant::now();
        l.granted(1, Duration::from_secs(30), sent);
        assert!(!l.frozen_at(sent + Duration::from_secs(14)));
        assert!(l.frozen_at(sent + Duration::from_secs(15)));
    }

    #[test]
    fn lost_is_sticky_until_a_new_grant() {
        let l = fenced();
        let now = Instant::now();
        l.granted(4, Duration::from_secs(60), now);
        l.lost();
        assert_eq!(l.state(), LeaseState::Lost);
        assert!(l.frozen() && l.is_lost());
        l.renewed(4, Instant::now());
        assert_eq!(
            l.state(),
            LeaseState::Lost,
            "a stray renewal does not revive a lost lease"
        );
        l.granted(5, Duration::from_secs(60), Instant::now());
        assert!(!l.frozen());
        assert_eq!(l.epoch(), 5);
    }

    /// Desktops are never frozen by their own lease; a hub without leases
    /// freezes nobody.
    #[test]
    fn unfenced_and_unleased_never_freeze() {
        let desktop = Lease::new();
        desktop.claim();
        desktop.lost();
        assert!(!desktop.frozen());

        let l = fenced();
        l.claim();
        l.granted(0, Duration::from_secs(60), Instant::now());
        assert_eq!(l.state(), LeaseState::Unleased);
        assert!(!l.frozen());
    }

    /// Only a drained process that holds the lease hands it back.
    #[test]
    fn release_hands_back_only_a_held_lease() {
        let l = fenced();
        l.claim();
        l.release();
        assert_eq!(l.release_epoch(), None, "nothing held, nothing to hand back");
        l.granted(6, Duration::from_secs(60), Instant::now());
        assert_eq!(l.release_epoch(), Some(6));

        let running = fenced();
        running.granted(2, Duration::from_secs(60), Instant::now());
        assert_eq!(running.release_epoch(), None, "a running process keeps its lease");
    }

    #[test]
    fn stamps_round_trip() {
        let l = Lease::new();
        let at = Instant::now() + Duration::from_millis(1234);
        let back = l.instant_of(l.stamp(at));
        assert!(back <= at && at - back < Duration::from_millis(1));
    }

    #[tokio::test]
    async fn waiters_wake_on_a_grant() {
        let l: &'static Lease = Box::leak(Box::new(Lease::new()));
        l.claim();
        let waiter = tokio::spawn(l.granted_or_unleased());
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        l.granted(1, Duration::from_secs(60), Instant::now());
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("woke")
            .expect("joined");
    }

    /// A refusal wakes whoever waits to ask again (the reconnect watcher),
    /// and leaves a waiter for a grant waiting.
    #[tokio::test]
    async fn a_refusal_wakes_the_askers() {
        let l: &'static Lease = Box::leak(Box::new(Lease::new()));
        l.claim();
        let refused = tokio::spawn(l.until_lost());
        let granted = tokio::spawn(l.granted_or_unleased());
        tokio::task::yield_now().await;
        assert!(!refused.is_finished());
        l.lost();
        tokio::time::timeout(Duration::from_secs(1), refused)
            .await
            .expect("woke")
            .expect("joined");
        tokio::task::yield_now().await;
        assert!(!granted.is_finished());
    }
}
