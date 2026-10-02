//! One push per burst of asks.
//!
//! Every ask files its Inbox row on this bot at once. Its copy on the hub,
//! which is what pushes the owner's phone (one push per new hub item), waits
//! a short window so a burst becomes one push. Live 2026-10-01: one
//! scheduled run raised 18 asks in 18 seconds, and each one was a push.
//!
//! The window opens with the first ask of a burst and closes [`WINDOW`]
//! later, whatever arrives in between (it is not pushed back, so a steady
//! trickle still reaches the owner every [`WINDOW`]). When it closes, the
//! asks still open in it go out as ONE hub item:
//! - one ask: its own item, with its answers and the chat it came from, so
//!   a tap opens its card there (or the Inbox when no chat raised it);
//! - several: one item, "{who}: N decisions waiting", whose tap opens this
//!   bot's Inbox. Its id names the newest ask in it, so the same backlog
//!   pushed again (a reconnect) is the same hub item and pushes nothing.
//!
//! An ask answered before its window closes is never pushed. A batch item is
//! cleared on the hub once every ask in it is settled.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// How long a burst gathers before the phone is pushed: long enough for one
/// run's burst of calls to land together, short enough that a single ask
/// still reaches the owner within seconds.
pub(crate) const WINDOW: Duration = Duration::from_secs(20);

/// The hub id prefix of a batch item.
pub(crate) const BATCH_PREFIX: &str = "permission-asks:";

/// One ask waiting for its push.
#[derive(Debug, Clone)]
pub(crate) struct Queued {
    pub ask_id: String,
    pub employee: String,
    /// The bot's own name, for a batch from several employees.
    pub bot: String,
    pub sentence: String,
    /// The ask's own hub item, sent as it is when it goes out alone.
    pub item: serde_json::Value,
}

type Sender = Arc<dyn Fn(serde_json::Value) + Send + Sync>;

/// The gathering of asks into pushes. One per bot (one per process).
pub(crate) struct AskPushes {
    window: Duration,
    send: Sender,
    pending: Mutex<Vec<Queued>>,
    /// Batch item id → the asks in it still open.
    batches: Mutex<HashMap<String, HashSet<String>>>,
}

impl AskPushes {
    pub(crate) fn new(window: Duration, send: Sender) -> Arc<Self> {
        Arc::new(Self { window, send, pending: Mutex::new(Vec::new()), batches: Mutex::new(HashMap::new()) })
    }

    /// Queue the asks for the current window; the first of a burst opens
    /// it. An ask already waiting is not queued twice.
    pub(crate) fn queue(self: &Arc<Self>, asks: Vec<Queued>) {
        if asks.is_empty() {
            return;
        }
        let opened = {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            let was_empty = pending.is_empty();
            for q in asks {
                if !pending.iter().any(|p| p.ask_id == q.ask_id) {
                    pending.push(q);
                }
            }
            was_empty && !pending.is_empty()
        };
        if opened && let Ok(rt) = tokio::runtime::Handle::try_current() {
            let me = self.clone();
            rt.spawn(async move {
                tokio::time::sleep(me.window).await;
                me.flush();
            });
        }
    }

    /// A blocking ask (a run is parked on the answer): its own item, out at
    /// once, never held for a burst — the owner is told now, wherever he
    /// is, and answers it from the notification itself.
    pub(crate) fn now(&self, ask: Queued) {
        (self.send)(ask.item);
    }

    /// Close the window: what is still waiting goes out as one hub item.
    pub(crate) fn flush(&self) {
        let asks = std::mem::take(&mut *self.pending.lock().unwrap_or_else(|e| e.into_inner()));
        match asks.as_slice() {
            [] => {}
            [one] => (self.send)(one.item.clone()),
            many => {
                let item = batch_item(many);
                let id = item["id"].as_str().unwrap_or_default().to_string();
                self.batches
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(id, many.iter().map(|q| q.ask_id.clone()).collect());
                (self.send)(item);
            }
        }
    }

    /// An ask was answered or withdrawn: never pushed if it was still
    /// waiting, and the batch it went out in is cleared once all of its
    /// asks are settled.
    pub(crate) fn settled(&self, ask_id: &str) {
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).retain(|q| q.ask_id != ask_id);
        let done: Vec<String> = {
            let mut batches = self.batches.lock().unwrap_or_else(|e| e.into_inner());
            for asks in batches.values_mut() {
                asks.remove(ask_id);
            }
            let done = batches.iter().filter(|(_, a)| a.is_empty()).map(|(id, _)| id.clone()).collect();
            batches.retain(|_, a| !a.is_empty());
            done
        };
        for id in done {
            (self.send)(serde_json::json!({ "id": id, "resolved": true }));
        }
    }
}

/// The one hub item for several asks: who is waiting, how many, a few of
/// them, and a link to this bot's Inbox.
pub(crate) fn batch_item(asks: &[Queued]) -> serde_json::Value {
    let first = &asks[0];
    let one_employee = asks.iter().all(|q| q.employee == first.employee);
    let who = if one_employee { &first.employee } else { &first.bot };
    let shown: Vec<String> = asks.iter().take(3).map(|q| upper_first(&q.sentence)).collect();
    let more = asks.len().saturating_sub(shown.len());
    let mut body = shown.join("; ");
    if more > 0 {
        body.push_str(&format!("; and {more} more."));
    } else {
        body.push('.');
    }
    let newest = &asks[asks.len() - 1].ask_id;
    serde_json::json!({
        "id": format!("{BATCH_PREFIX}{newest}"),
        "type": "permission_asks",
        "title": format!("{who}: {} decisions waiting", asks.len()),
        "body": body,
        "link": "/inbox",
    })
}

fn upper_first(s: &str) -> String {
    let s = s.trim().trim_end_matches('.');
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// This bot's gathering, sending through the hub Inbox push.
pub(crate) fn global(state: &crate::state::AppState) -> &'static Arc<AskPushes> {
    static PUSHES: OnceLock<Arc<AskPushes>> = OnceLock::new();
    PUSHES.get_or_init(|| {
        let state = state.clone();
        AskPushes::new(WINDOW, Arc::new(move |item| crate::codes::push_inbox(&state, item)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What reached the hub, and how many phone pushes that was: the hub
    /// pushes on the first store of an item id only.
    #[derive(Default)]
    struct Hub {
        items: Mutex<Vec<serde_json::Value>>,
        seen: Mutex<HashSet<String>>,
        pushes: Mutex<usize>,
    }

    /// The window, scaled down: a burst of 18 spread over it.
    const W: Duration = Duration::from_millis(300);

    async fn settle() {
        tokio::time::sleep(W + Duration::from_millis(200)).await;
    }

    fn rig(window: Duration) -> (Arc<AskPushes>, Arc<Hub>) {
        let hub = Arc::new(Hub::default());
        let h = hub.clone();
        let pushes = AskPushes::new(
            window,
            Arc::new(move |item: serde_json::Value| {
                let id = item["id"].as_str().unwrap_or_default().to_string();
                if item["resolved"] != true && h.seen.lock().unwrap().insert(id) {
                    *h.pushes.lock().unwrap() += 1;
                }
                h.items.lock().unwrap().push(item);
            }),
        );
        (pushes, hub)
    }

    fn ask(i: usize, employee: &str) -> Queued {
        Queued {
            ask_id: format!("ask-{i:02}"),
            employee: employee.into(),
            bot: "Kestrel".into(),
            sentence: format!("sending card notice {i:02}"),
            item: serde_json::json!({ "id": format!("permission-ask:ask-{i:02}"), "type": "permission_ask", "title": "t", "link": "/inbox?m=x" }),
        }
    }

    /// Live 2026-10-01: a scheduled run raised 18 asks in 18 seconds. One
    /// push, "…: 18 decisions waiting", whose tap opens the bot's Inbox.
    #[tokio::test]
    async fn eighteen_asks_in_a_burst_are_one_push() {
        let (pushes, hub) = rig(W);
        for i in 0..18 {
            pushes.queue(vec![ask(i, "Accounts Receivable Specialist")]);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        settle().await;
        assert_eq!(*hub.pushes.lock().unwrap(), 1);
        let items = hub.items.lock().unwrap().clone();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["title"], "Accounts Receivable Specialist: 18 decisions waiting");
        assert_eq!(items[0]["type"], "permission_asks");
        assert_eq!(items[0]["link"], "/inbox");
        assert_eq!(items[0]["id"], "permission-asks:ask-17");
        assert!(items[0]["body"].as_str().unwrap().ends_with("and 15 more."), "{}", items[0]["body"]);
    }

    /// One ask alone goes out as itself: its answers and its place.
    #[tokio::test]
    async fn a_lone_ask_is_its_own_push() {
        let (pushes, hub) = rig(W);
        pushes.queue(vec![ask(1, "Ava")]);
        settle().await;
        let items = hub.items.lock().unwrap().clone();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], "permission-ask:ask-01");
    }

    /// Answered in its chat before the window closed: no push at all.
    #[tokio::test]
    async fn an_ask_answered_inside_the_window_is_never_pushed() {
        let (pushes, hub) = rig(W);
        pushes.queue(vec![ask(1, "Ava")]);
        pushes.settled("ask-01");
        settle().await;
        assert!(hub.items.lock().unwrap().is_empty());
    }

    /// Several employees: the bot's name. A batch clears on the hub once
    /// every ask in it is settled.
    #[tokio::test]
    async fn a_batch_names_the_bot_and_clears_when_all_are_settled() {
        let (pushes, hub) = rig(W);
        pushes.queue(vec![ask(1, "Ava"), ask(2, "Sam")]);
        settle().await;
        assert_eq!(hub.items.lock().unwrap()[0]["title"], "Kestrel: 2 decisions waiting");
        pushes.settled("ask-01");
        assert_eq!(hub.items.lock().unwrap().len(), 1, "one still open");
        pushes.settled("ask-02");
        let items = hub.items.lock().unwrap().clone();
        assert_eq!(items[1], serde_json::json!({ "id": "permission-asks:ask-02", "resolved": true }));
    }

    /// Live 2026-10-02: Bookkeeper's workflow step parked on the owner's OK.
    /// A blocking ask is pushed at once, on its own, even while a burst is
    /// gathering; the burst still goes out as one push after it.
    #[tokio::test]
    async fn a_blocking_ask_is_pushed_at_once_on_its_own() {
        let (pushes, hub) = rig(W);
        pushes.queue(vec![ask(1, "Ava"), ask(2, "Ava")]);
        let mut bk = ask(3, "Bookkeeper");
        bk.item["blocking"] = serde_json::json!(true);
        pushes.now(bk);
        {
            let items = hub.items.lock().unwrap();
            assert_eq!(items.len(), 1, "out before the window closes");
            assert_eq!(items[0]["id"], "permission-ask:ask-03");
            assert_eq!(items[0]["blocking"], true);
        }
        settle().await;
        let items = hub.items.lock().unwrap().clone();
        assert_eq!(items.len(), 2);
        assert_eq!(items[1]["title"], "Ava: 2 decisions waiting", "never part of the burst");
        assert_eq!(*hub.pushes.lock().unwrap(), 2);
    }

    /// A bot with 29 asks already open when it upgrades: its first sync is
    /// ONE push for the backlog, and a later sync of the same backlog is the
    /// same hub item, so no push at all. Nothing is declined or withdrawn.
    #[tokio::test]
    async fn a_backlog_of_twenty_nine_is_one_push_and_a_resync_is_none() {
        let (pushes, hub) = rig(W);
        let backlog: Vec<Queued> = (0..29).map(|i| ask(i, "Kestrel General Manager")).collect();
        pushes.queue(backlog.clone());
        settle().await;
        assert_eq!(*hub.pushes.lock().unwrap(), 1);
        pushes.queue(backlog);
        settle().await;
        assert_eq!(*hub.pushes.lock().unwrap(), 1, "the same backlog again pushes nothing");
        assert!(hub.items.lock().unwrap().iter().all(|i| i["resolved"] != true), "nothing settled");
    }
}
