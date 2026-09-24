//! The one reminder path. Everything the runtime tells the model mid-turn is
//! queued here and, just before the next call, persisted append-only in the
//! transcript as a typed attachment row: role `user`, metadata
//! `{"attachment": {"kind": "<name>"}, "isMeta": true}`, text wrapped in
//! `<system-reminder>`. The row stays where it was delivered and is re-sent
//! on every later call like any other message; it is hidden from the owner's
//! thread and never rewritten.
//!
//! Facts always ride. Steering — text that tells the model how to behave
//! next — rides only when `NEBO_STEERING` lets its name through, and every
//! decision is counted.
//!
//! `NEBO_STEERING`: unset / `on` = every steering name on; `off` = all off;
//! `a,b` = only those on; `-a,-b` = all on except those. Read once per
//! process.

use std::collections::{BTreeMap, BTreeSet};

use types::NeboError;

pub use crate::steering::wrap_system_reminder as wrap;

/// A reminder's kind: a fact always rides; steering passes the switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Fact,
    Steering,
}

/// One reminder as the transcript stores it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentRow {
    /// The reminder's name (the event table's name).
    pub kind: &'static str,
    /// The `<system-reminder>`-wrapped text.
    pub content: String,
}

impl AttachmentRow {
    /// The row's metadata: typed, and meta so the owner's thread hides it.
    pub fn metadata(&self) -> serde_json::Value {
        serde_json::json!({ "attachment": { "kind": self.kind }, "isMeta": true })
    }

    /// The row as the model receives it.
    pub fn to_message(&self) -> ai::Message {
        ai::Message {
            role: "user".to_string(),
            content: self.content.clone(),
            ..Default::default()
        }
    }
}

/// Where attachment rows are written: the turn's conversation, append-only.
/// WP2.1 implements it over the chat store, bound to the turn's chat.
pub trait AttachmentStore {
    fn append_attachment(&self, row: &AttachmentRow) -> Result<(), NeboError>;
}

/// A session's conversation: rows go through the session's one append path.
pub struct SessionTranscript<'a> {
    pub sessions: &'a crate::session::SessionManager,
    pub session_id: &'a str,
}

impl AttachmentStore for SessionTranscript<'_> {
    fn append_attachment(&self, row: &AttachmentRow) -> Result<(), NeboError> {
        self.sessions
            .append_message(
                self.session_id,
                "user",
                &row.content,
                None,
                None,
                Some(&row.metadata().to_string()),
            )
            .map(|_| ())
    }
}

#[derive(Debug)]
struct Queued {
    row: AttachmentRow,
    /// Already written by an earlier `attach` whose call has not landed.
    stored: bool,
}

/// The reminders queued for the next call, and the turn's steering tally.
#[derive(Debug, Default)]
pub struct Reminders {
    queued: Vec<Queued>,
    /// Per steering name: (fired, suppressed by the switch).
    tally: BTreeMap<&'static str, (u32, u32)>,
}

impl Reminders {
    /// Queue a fact. The switch never touches facts.
    pub fn fact(&mut self, name: &'static str, text: impl Into<String>) {
        self.enqueue(name, &text.into());
    }

    /// Queue the steering named `name` unless `NEBO_STEERING` holds it back;
    /// counts either way. Returns whether it was queued.
    pub fn steer(&mut self, name: &'static str, text: &str) -> bool {
        self.steer_under(&SPEC, name, text)
    }

    fn steer_under(&mut self, spec: &Spec, name: &'static str, text: &str) -> bool {
        let on = spec.allows(name);
        let entry = self.tally.entry(name).or_default();
        if on {
            entry.0 += 1;
            self.enqueue(name, text);
        } else {
            entry.1 += 1;
        }
        on
    }

    /// A reminder already waiting with the same text is not queued twice.
    fn enqueue(&mut self, name: &'static str, text: &str) {
        let content = wrap(text);
        if self.queued.iter().any(|q| q.row.content == content) {
            return;
        }
        self.queued.push(Queued {
            row: AttachmentRow {
                kind: name,
                content,
            },
            stored: false,
        });
    }

    /// The only way reminders enter a call: persist every queued reminder
    /// not yet written, in queue order, and return the rows written now so
    /// the caller appends them to the request it already loaded. A retry of
    /// a call that did not land writes nothing again: its rows are already
    /// in the conversation it reloads. On a store error the rows written so
    /// far stay marked and the rest are tried again by the next `attach`.
    pub fn attach(&mut self, store: &dyn AttachmentStore) -> Result<Vec<ai::Message>, NeboError> {
        let mut written = Vec::new();
        for q in self.queued.iter_mut().filter(|q| !q.stored) {
            store.append_attachment(&q.row)?;
            q.stored = true;
            written.push(q.row.to_message());
        }
        Ok(written)
    }

    /// The call carrying the queue reached the model: the queue is empty.
    pub fn landed(&mut self) {
        self.queued.clear();
    }

    /// `name:fired=N,suppressed=M` per steering name decided this turn, for
    /// the per-turn log line; `none` when no steering tripped.
    pub fn tally(&self) -> String {
        if self.tally.is_empty() {
            return "none".to_string();
        }
        self.tally
            .iter()
            .map(|(name, (fired, suppressed))| {
                format!("{name}:fired={fired},suppressed={suppressed}")
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// A parsed `NEBO_STEERING` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spec {
    All,
    Only(BTreeSet<String>),
    Except(BTreeSet<String>),
}

impl Spec {
    pub fn parse(raw: Option<&str>) -> Spec {
        let raw = raw.unwrap_or("").trim().to_ascii_lowercase();
        match raw.as_str() {
            "" | "on" | "1" | "true" | "yes" => return Spec::All,
            "off" | "0" | "false" | "no" => return Spec::Only(BTreeSet::new()),
            _ => {}
        }
        let mut on = BTreeSet::new();
        let mut except = BTreeSet::new();
        for item in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            match item.strip_prefix('-') {
                Some(name) => except.insert(name.trim().to_string()),
                None => on.insert(item.to_string()),
            };
        }
        if on.is_empty() {
            Spec::Except(except)
        } else {
            Spec::Only(&on - &except)
        }
    }

    pub fn allows(&self, name: &str) -> bool {
        match self {
            Spec::All => true,
            Spec::Only(on) => on.contains(name),
            Spec::Except(off) => !off.contains(name),
        }
    }

    /// Names in the spec that address no steering.
    pub fn unknown(&self) -> Vec<String> {
        let listed = match self {
            Spec::All => return Vec::new(),
            Spec::Only(s) | Spec::Except(s) => s,
        };
        listed
            .iter()
            .filter(|n| !names().contains(&n.as_str()))
            .cloned()
            .collect()
    }
}

/// Every name `NEBO_STEERING` can address: the steering rows of the event
/// table.
pub fn names() -> Vec<&'static str> {
    super::events::STEERING_NAMES.to_vec()
}

static SPEC: std::sync::LazyLock<Spec> =
    std::sync::LazyLock::new(|| Spec::parse(std::env::var("NEBO_STEERING").ok().as_deref()));

/// The one steering predicate: may the steering named `name` run?
pub fn enabled(name: &str) -> bool {
    SPEC.allows(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// An in-memory transcript; `fail_after` makes the Nth write fail.
    #[derive(Default)]
    struct Transcript {
        rows: Mutex<Vec<AttachmentRow>>,
        fail_after: Option<usize>,
    }

    impl AttachmentStore for Transcript {
        fn append_attachment(&self, row: &AttachmentRow) -> Result<(), NeboError> {
            let mut rows = self.rows.lock().unwrap();
            if self.fail_after.is_some_and(|n| rows.len() >= n) {
                return Err(NeboError::Database("disk full".into()));
            }
            rows.push(row.clone());
            Ok(())
        }
    }

    impl Transcript {
        fn kinds(&self) -> Vec<&'static str> {
            self.rows.lock().unwrap().iter().map(|r| r.kind).collect()
        }
    }

    #[test]
    fn spec_parses_on_off_lists_and_exceptions() {
        for on in [
            None,
            Some(""),
            Some("on"),
            Some("ON"),
            Some(" 1 "),
            Some("true"),
            Some("yes"),
        ] {
            assert_eq!(Spec::parse(on), Spec::All, "{on:?}");
        }
        for off in ["off", "OFF", "0", "false", "no"] {
            let spec = Spec::parse(Some(off));
            assert!(
                names().iter().all(|n| !spec.allows(n)),
                "{off}: nothing runs"
            );
            assert!(!spec.allows("anything"));
            assert!(spec.unknown().is_empty());
        }
        let only = Spec::parse(Some(" app_steering , other "));
        assert!(only.allows("app_steering") && only.allows("other"));
        assert!(!only.allows("third"));
        let except = Spec::parse(Some("-app_steering"));
        assert!(!except.allows("app_steering") && except.allows("other"));
        let mixed = Spec::parse(Some("app_steering,other,-other"));
        assert!(mixed.allows("app_steering") && !mixed.allows("other"));
    }

    #[test]
    fn spec_reports_names_that_address_nothing() {
        assert!(Spec::All.unknown().is_empty());
        assert!(Spec::parse(Some("app_steering")).unknown().is_empty());
        assert_eq!(
            Spec::parse(Some("app_steering,no_such_thing,-also_not")).unknown(),
            vec!["no_such_thing".to_string()],
            "an excepted name drops out of an Only list; the unknown listed one is reported"
        );
        assert_eq!(
            Spec::parse(Some("-nope")).unknown(),
            vec!["nope".to_string()]
        );
    }

    #[test]
    fn facts_ignore_the_switch() {
        let mut r = Reminders::default();
        r.fact("message_time", "Message sent at noon.");
        let store = Transcript::default();
        let sent = r.attach(&store).unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].content, wrap("Message sent at noon."));
        assert_eq!(r.tally(), "none", "facts are never counted as steering");
    }

    #[test]
    fn steering_off_queues_nothing_and_counts() {
        let off = Spec::parse(Some("off"));
        let mut r = Reminders::default();
        assert!(!r.steer_under(&off, "app_steering", "do the thing"));
        assert!(!r.steer_under(&off, "app_steering", "do the thing"));
        let store = Transcript::default();
        assert!(r.attach(&store).unwrap().is_empty());
        assert!(store.kinds().is_empty());
        assert_eq!(r.tally(), "app_steering:fired=0,suppressed=2");
    }

    #[test]
    fn steering_on_queues_and_counts() {
        let mut r = Reminders::default();
        assert!(r.steer_under(&Spec::All, "app_steering", "do the thing"));
        assert!(!r.steer_under(&Spec::parse(Some("-app_steering")), "app_steering", "again"));
        let store = Transcript::default();
        let sent = r.attach(&store).unwrap();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].content.contains("do the thing"));
        assert_eq!(r.tally(), "app_steering:fired=1,suppressed=1");
    }

    #[test]
    fn attach_persists_typed_meta_rows_in_order() {
        let mut r = Reminders::default();
        r.fact("message_time", "one");
        r.fact("relevant_memories", "two");
        let store = Transcript::default();
        let sent = r.attach(&store).unwrap();
        assert_eq!(store.kinds(), vec!["message_time", "relevant_memories"]);
        assert_eq!(
            sent.iter().map(|m| m.content.clone()).collect::<Vec<_>>(),
            [wrap("one"), wrap("two")]
        );
        assert!(sent.iter().all(|m| m.role == "user"));
        let row = &store.rows.lock().unwrap()[0];
        assert_eq!(
            row.metadata(),
            serde_json::json!({"attachment": {"kind": "message_time"}, "isMeta": true})
        );
        assert!(row.content.starts_with("<system-reminder>"));
    }

    #[test]
    fn a_waiting_reminder_is_queued_once() {
        let mut r = Reminders::default();
        r.fact("a", "same");
        r.fact("a", "same");
        r.fact("b", "other");
        let store = Transcript::default();
        assert_eq!(r.attach(&store).unwrap().len(), 2);
        assert_eq!(store.kinds(), vec!["a", "b"]);
    }

    #[test]
    fn a_retry_reuses_stored_rows_and_landed_clears() {
        let mut r = Reminders::default();
        r.fact("cutoff_resume", "continue");
        let store = Transcript::default();
        assert_eq!(r.attach(&store).unwrap().len(), 1);
        // The call failed before landing; its retry reloads the conversation
        // (the row is in it) and writes nothing again. A reminder queued
        // meanwhile is written once, and the stored one is not re-queued.
        r.fact("cutoff_resume", "continue");
        r.fact("threshold", "Context 82% full.");
        let retry = r.attach(&store).unwrap();
        assert_eq!(retry.len(), 1);
        assert_eq!(store.kinds(), vec!["cutoff_resume", "threshold"]);
        r.landed();
        assert!(r.attach(&store).unwrap().is_empty());
        // After landing, the same text is a new delivery.
        r.fact("cutoff_resume", "continue");
        assert_eq!(r.attach(&store).unwrap().len(), 1);
        assert_eq!(store.kinds().len(), 3);
    }

    #[test]
    fn a_failed_write_is_tried_again() {
        let mut r = Reminders::default();
        r.fact("a", "one");
        r.fact("b", "two");
        let failing = Transcript {
            fail_after: Some(1),
            ..Default::default()
        };
        assert!(r.attach(&failing).is_err());
        assert_eq!(failing.kinds(), vec!["a"]);
        let healthy = Transcript::default();
        let sent = r.attach(&healthy).unwrap();
        assert_eq!(sent.len(), 1, "the written row is not written twice");
        assert_eq!(healthy.kinds(), vec!["b"]);
    }
}
