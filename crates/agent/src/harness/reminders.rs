//! The one reminder path. Everything the runtime tells the model mid-turn is
//! an event (`events.rs`); the event table turns it into an attachment, and
//! this module persists it append-only in the conversation as a typed
//! attachment row:
//!
//! - role `user`, text wrapped in `<system-reminder>`;
//! - metadata `{"attachment": {"kind": "<name>", ...}, "isMeta": true}`.
//!
//! The row is written where it was delivered and never rewritten. The model
//! gets it on every later call because the conversation is reloaded from the
//! store each step; rows below a checkpoint boundary drop out with the rest
//! of that history. `isMeta` hides it from the owner's thread.
//!
//! Only this module writes attachment rows (a convention, as in Claude
//! Code; the store has no guard).
//!
//! Loading: the legacy history loader (`session::is_stored_steering`) still
//! drops every `isMeta` row that starts with `<system-reminder>`, which
//! includes these. The new loader (WP2.3, at cutover) keeps rows for which
//! [`attachment_kind`] is `Some` and deletes the legacy filter; until then
//! nothing writes attachment rows.
//!
//! `NEBO_STEERING` switches named attachments off for measurement; every
//! attachment is on in the build. Unset / `on` = all on; `off` = all off;
//! `a,b` = only those on; `-a,-b` = all on except those. Read once per
//! process.

use std::collections::{BTreeMap, BTreeSet};

use db::models::ChatMessage;
use types::NeboError;

use super::events::{self, TurnEvent};
use crate::session::SessionManager;

pub use crate::steering::wrap_system_reminder as wrap;

/// The metadata key a typed attachment row carries its kind under.
const ATTACHMENT_KEY: &str = "attachment";

/// One attachment as the table builds it and the conversation stores it.
#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    /// The table's name for it (`events::NAMES`).
    pub kind: &'static str,
    /// The text, unwrapped.
    pub text: String,
    /// Structured fields stored next to the kind (a listing's current set).
    pub data: serde_json::Map<String, serde_json::Value>,
}

impl Attachment {
    /// The row's metadata: typed, and meta so the owner's thread hides it.
    pub fn metadata(&self) -> serde_json::Value {
        let mut attachment = self.data.clone();
        attachment.insert("kind".into(), self.kind.into());
        serde_json::json!({ ATTACHMENT_KEY: attachment, "isMeta": true })
    }
}

/// The kind of a stored typed attachment row, `None` for every other row
/// (including legacy `<system-reminder>` rows, which carry no kind).
pub fn attachment_kind(msg: &ChatMessage) -> Option<String> {
    attachment_fields(msg)?
        .get("kind")
        .and_then(|k| k.as_str())
        .map(str::to_string)
}

/// The `attachment` object of a stored typed attachment row.
pub(crate) fn attachment_fields(msg: &ChatMessage) -> Option<serde_json::Map<String, serde_json::Value>> {
    if msg.role != "user" {
        return None;
    }
    let meta: serde_json::Value = serde_json::from_str(msg.metadata.as_deref()?).ok()?;
    match meta.get(ATTACHMENT_KEY)? {
        serde_json::Value::Object(fields) if fields.contains_key("kind") => Some(fields.clone()),
        _ => None,
    }
}

/// The attachments waiting for the next call, and the turn's tally.
#[derive(Debug, Default)]
pub struct Reminders {
    queued: Vec<Attachment>,
    /// Per name: (written, switched off).
    tally: BTreeMap<&'static str, (u32, u32)>,
}

impl Reminders {
    /// Queue the attachment an event makes, unless the table has nothing to
    /// say or `NEBO_STEERING` switches its name off (counted).
    pub fn add(&mut self, event: &TurnEvent) {
        self.add_under(&SPEC, event);
    }

    fn add_under(&mut self, spec: &Spec, event: &TurnEvent) {
        let Some(attachment) = events::attachment_for(event) else {
            return;
        };
        if !spec.allows(attachment.kind) {
            self.tally.entry(attachment.kind).or_default().1 += 1;
            return;
        }
        if self.queued.contains(&attachment) {
            return;
        }
        self.queued.push(attachment);
    }

    /// The only way an attachment enters the context: persist each queued
    /// attachment as a typed row at the end of the session's conversation,
    /// in queue order, and empty the queue. The step then loads the
    /// conversation, rows included; a retried call reloads the same rows and
    /// writes nothing again. On a store error the unwritten rest stays queued
    /// for the next write.
    pub fn write(&mut self, sessions: &SessionManager, session_id: &str) -> Result<(), NeboError> {
        while let Some(attachment) = self.queued.first() {
            let metadata = attachment.metadata().to_string();
            sessions.append_message(session_id, "user", &wrap(&attachment.text), None, None, Some(&metadata))?;
            let written = self.queued.remove(0);
            self.tally.entry(written.kind).or_default().0 += 1;
        }
        Ok(())
    }

    /// `name:written=N,switched_off=M` per name this turn, for the per-turn
    /// line; `none` when nothing was attached or switched off.
    pub fn tally(&self) -> String {
        if self.tally.is_empty() {
            return "none".to_string();
        }
        self.tally
            .iter()
            .map(|(name, (written, off))| format!("{name}:written={written},switched_off={off}"))
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

    /// Names in the spec that address no attachment.
    pub fn unknown(&self) -> Vec<String> {
        let listed = match self {
            Spec::All => return Vec::new(),
            Spec::Only(s) | Spec::Except(s) => s,
        };
        listed
            .iter()
            .filter(|n| !events::NAMES.contains(&n.as_str()))
            .cloned()
            .collect()
    }
}

static SPEC: std::sync::LazyLock<Spec> = std::sync::LazyLock::new(|| {
    let spec = Spec::parse(std::env::var("NEBO_STEERING").ok().as_deref());
    let unknown = spec.unknown();
    if !unknown.is_empty() {
        tracing::warn!(?unknown, "NEBO_STEERING names no attachment");
    }
    spec
});

/// The one switch predicate: may the attachment named `name` be written?
pub fn enabled(name: &str) -> bool {
    SPEC.allows(name)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::harness::events::Threshold;
    use crate::harness::tool_surface::ListingDelta;

    struct Conversation {
        store: Arc<db::Store>,
        sessions: SessionManager,
        session_id: String,
        chat_id: String,
    }

    impl Conversation {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("nebo-reminders-{}.db", uuid::Uuid::new_v4()));
            let store = Arc::new(db::Store::new(path.to_str().unwrap()).expect("test store"));
            let sessions = SessionManager::new(store.clone());
            let session_id = sessions.get_or_create("agent:a1:web", "").expect("session").id;
            let chat_id = sessions.active_chat_id(&session_id);
            Conversation { store, sessions, session_id, chat_id }
        }

        fn say(&self, role: &str, text: &str) {
            self.sessions.append_message(&self.session_id, role, text, None, None, None).expect("append");
        }

        fn write(&self, r: &mut Reminders) {
            r.write(&self.sessions, &self.session_id).expect("write");
        }

        /// What a step loads: the conversation since the last boundary.
        fn load(&self) -> Vec<ChatMessage> {
            self.store.get_chat_messages(&self.chat_id).expect("load")
        }

        fn kinds(&self) -> Vec<String> {
            self.load().iter().filter_map(attachment_kind).collect()
        }
    }

    fn usage(percent_full: u8) -> TurnEvent {
        TurnEvent::Usage(Threshold::Context { percent_full })
    }

    #[test]
    fn spec_parses_on_off_lists_and_exceptions() {
        for on in [None, Some(""), Some("on"), Some("ON"), Some(" 1 "), Some("true"), Some("yes")] {
            assert_eq!(Spec::parse(on), Spec::All, "{on:?}");
        }
        for off in ["off", "OFF", "0", "false", "no"] {
            let spec = Spec::parse(Some(off));
            assert!(events::NAMES.iter().all(|n| !spec.allows(n)), "{off}: nothing is written");
            assert!(spec.unknown().is_empty());
        }
        let only = Spec::parse(Some(" usage , app_hook "));
        assert!(only.allows("usage") && only.allows("app_hook"));
        assert!(!only.allows("task_reminder"));
        let except = Spec::parse(Some("-usage"));
        assert!(!except.allows("usage") && except.allows("app_hook"));
        let mixed = Spec::parse(Some("usage,app_hook,-app_hook"));
        assert!(mixed.allows("usage") && !mixed.allows("app_hook"));
    }

    #[test]
    fn spec_reports_names_that_address_nothing() {
        assert!(Spec::All.unknown().is_empty());
        assert!(Spec::parse(Some("task_reminder")).unknown().is_empty());
        assert_eq!(
            Spec::parse(Some("usage,no_such_thing,-also_not")).unknown(),
            vec!["no_such_thing".to_string()],
            "an excepted name drops out of an Only list; the unknown listed one is reported"
        );
        assert_eq!(Spec::parse(Some("-nope")).unknown(), vec!["nope".to_string()]);
    }

    #[test]
    fn attachment_row_is_typed_meta_and_wrapped() {
        let c = Conversation::new();
        let mut r = Reminders::default();
        r.add(&usage(82));
        c.write(&mut r);
        let rows = c.load();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.role, "user");
        assert_eq!(row.content, wrap("Context 82% full."));
        assert!(row.content.starts_with("<system-reminder>"));
        let meta: serde_json::Value = serde_json::from_str(row.metadata.as_deref().unwrap()).unwrap();
        assert_eq!(meta, serde_json::json!({"attachment": {"kind": "usage"}, "isMeta": true}));
        assert_eq!(attachment_kind(row).as_deref(), Some("usage"));
    }

    /// A legacy stored reminder (isMeta, wrapped, no kind) is not a typed
    /// attachment: the new loader tells them apart by kind alone.
    #[test]
    fn legacy_reminder_rows_have_no_kind() {
        let c = Conversation::new();
        c.sessions
            .append_message(&c.session_id, "user", &wrap("Team \"Ops\""), None, None, Some(r#"{"isMeta":true}"#))
            .unwrap();
        c.say("user", "hello");
        assert!(c.load().iter().all(|m| attachment_kind(m).is_none()));
    }

    #[test]
    fn attachment_written_once_and_resent_on_later_calls() {
        let c = Conversation::new();
        let mut r = Reminders::default();
        c.say("user", "Draft the plan");
        r.add(&usage(82));
        c.write(&mut r);
        let first_call = c.load();
        c.say("assistant", "Working on it.");
        // Later steps write nothing new; every later load still carries the
        // row, at the place it was delivered.
        c.write(&mut r);
        c.say("user", "Go on");
        c.write(&mut r);
        let later_call = c.load();
        assert_eq!(c.kinds(), vec!["usage"], "written exactly once");
        let at = |rows: &[ChatMessage]| rows.iter().position(|m| attachment_kind(m).is_some());
        assert_eq!(at(&first_call), Some(1));
        assert_eq!(at(&later_call), Some(1), "re-sent where it was delivered");
        assert_eq!(later_call[1].content, first_call[1].content, "never rewritten");
        assert_eq!(r.tally(), "usage:written=1,switched_off=0");
    }

    #[test]
    fn retry_does_not_write_twice() {
        let c = Conversation::new();
        let mut r = Reminders::default();
        r.add(&TurnEvent::CutoffResume);
        r.add(&TurnEvent::CutoffResume);
        c.write(&mut r);
        // The call failed transiently: the step reloads and calls again
        // without writing; the reload already carries the row.
        c.write(&mut r);
        let retry = c.load();
        assert_eq!(c.kinds(), vec!["cutoff_resume"]);
        assert_eq!(retry.len(), 1);
        // A new event after the retry is a new delivery.
        r.add(&TurnEvent::CutoffResume);
        c.write(&mut r);
        assert_eq!(c.kinds(), vec!["cutoff_resume", "cutoff_resume"]);
    }

    #[test]
    fn attachments_before_boundary_are_not_loaded() {
        let c = Conversation::new();
        let mut r = Reminders::default();
        c.say("user", "Draft the plan");
        r.add(&usage(82));
        c.write(&mut r);
        let before = c.load().into_iter().find(|m| attachment_kind(m).is_some()).unwrap();
        c.store.compact_chat_history(&c.chat_id, &uuid::Uuid::new_v4().to_string(), "summary", None).unwrap();
        r.add(&TurnEvent::GoalSet("all tests pass".into()));
        c.write(&mut r);
        assert_eq!(c.kinds(), vec!["goal_set"], "the pre-boundary row is not loaded");
        assert!(c.store.get_chat_message(&before.id).unwrap().is_some(), "it stays on disk");
    }

    #[test]
    fn switch_off_writes_nothing_and_counts() {
        let c = Conversation::new();
        let off = Spec::parse(Some("off"));
        let mut r = Reminders::default();
        r.add_under(&off, &usage(82));
        r.add_under(&off, &usage(90));
        r.add_under(&Spec::parse(Some("-usage")), &TurnEvent::CutoffResume);
        c.write(&mut r);
        assert_eq!(c.kinds(), vec!["cutoff_resume"]);
        assert_eq!(
            r.tally(),
            "cutoff_resume:written=1,switched_off=0 usage:written=0,switched_off=2"
        );
        assert_eq!(Reminders::default().tally(), "none");
    }

    #[test]
    fn listing_rows_store_what_they_announced() {
        let c = Conversation::new();
        let mut r = Reminders::default();
        r.add(&TurnEvent::ToolsAvailable(ListingDelta::all(["mail_send".to_string()].into())));
        c.write(&mut r);
        let fields = attachment_fields(&c.load()[0]).unwrap();
        assert_eq!(fields["kind"], "tools_available");
        assert_eq!(fields["added"], serde_json::json!({"mail_send": ""}));
        assert_eq!(fields["removed"], serde_json::json!([]));
        assert_eq!(events::announced("tools_available", &c.load()).into_keys().collect::<Vec<_>>(), ["mail_send"]);
    }
}
