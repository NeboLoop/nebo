//! Turn events and THE table that turns each into an attachment. Every row
//! states a fact at the moment it became true, except the task reminder,
//! which fires on Claude Code's step counts. A new attachment is one row
//! here plus its producer.

use std::collections::{BTreeMap, BTreeSet};

use db::models::ChatMessage;

use super::reminders::{Attachment, attachment_fields};
use super::tool_surface::{ListingDelta as ToolsDelta, render_listing};

/// Something that happened that the model hears about on its next call.
#[derive(Debug, Clone)]
pub enum TurnEvent {
    /// The date rolled over mid-session; the date itself is in the system
    /// prompt.
    DateChanged(chrono::NaiveDate),
    /// Team roster, @mention, room briefing.
    RunBriefing(String),
    /// The outside-origin notice.
    RestrictedRun(String),
    /// Recalled memories, none already surfaced this session.
    RelevantMemories(Vec<crate::memory::ScoredMemory>),
    /// The read ledger's change notes.
    FilesChanged(Vec<String>),
    Diagnostics(Vec<String>),
    /// The task tools went unused; see [`task_reminder_due`].
    TasksIdle(Vec<WorkTaskLine>),
    PlanMode {
        entered: bool,
    },
    GoalSet(String),
    GoalCleared,
    /// The done check found the agreed goal unmet.
    GoalCheck {
        reason: String,
        condition: String,
    },
    Usage(Threshold),
    /// The listed deferred tools changed; names only (tools doc §4.1).
    ToolsAvailable(ToolsDelta),
    /// The skill set changed; a line is the skill's one-line description.
    SkillListing(LinedDelta),
    /// The helper types changed; a line is the type's use and tool set.
    HelperTypes(LinedDelta),
    /// A proactive inbox item or a presence change.
    BackgroundUpdate(String),
    /// The last reply hit the output limit.
    CutoffResume,
    /// The unmet workflow contract term (workflow mode only).
    WorkflowContract(String),
    /// The `steering.generate` app hook's text.
    AppHook {
        label: String,
        text: String,
    },
}

/// One work task as the task reminder lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkTaskLine {
    pub subject: String,
    pub status: String,
}

/// A limit the turn is nearing: the number only, never an instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Threshold {
    Context { percent_full: u8 },
    Spend { spent_microcents: i64, cap_microcents: i64 },
}

/// Every attachment name, what `NEBO_STEERING` can address.
pub const NAMES: &[&str] = &[
    "date_changed",
    "run_briefing",
    "restricted_run",
    "relevant_memories",
    "files_changed",
    "diagnostics",
    "task_reminder",
    "plan_mode",
    "goal_set",
    "goal_cleared",
    "goal_check",
    "usage",
    "tools_available",
    "skill_listing",
    "helper_types",
    "background_update",
    "cutoff_resume",
    "workflow_contract",
    "app_hook",
];

/// Most memories one recall surfaces.
const MAX_RECALLED: usize = 5;

const MICROCENTS_PER_DOLLAR: f64 = 100_000_000.0;

/// THE table: the attachment an event makes, or None when it has nothing to
/// say.
pub fn attachment_for(e: &TurnEvent) -> Option<Attachment> {
    let (kind, text) = match e {
        TurnEvent::DateChanged(date) => (
            "date_changed",
            format!("The date is now {}.", date.format("%A, %B %-d, %Y")),
        ),
        TurnEvent::RunBriefing(text) => ("run_briefing", non_empty(text)?),
        TurnEvent::RestrictedRun(text) => ("restricted_run", non_empty(text)?),
        TurnEvent::RelevantMemories(found) => {
            if found.is_empty() {
                return None;
            }
            let lines: Vec<String> = found
                .iter()
                .take(MAX_RECALLED)
                .map(|m| format!("- {}: {}", m.memory.key, m.memory.value))
                .collect();
            ("relevant_memories", format!("Memories that may apply:\n{}", lines.join("\n")))
        }
        TurnEvent::FilesChanged(notes) => ("files_changed", non_empty(&notes.join("\n"))?),
        TurnEvent::Diagnostics(notes) => ("diagnostics", non_empty(&notes.join("\n"))?),
        TurnEvent::TasksIdle(tasks) => {
            let mut text = "The task list hasn't been touched in a while. If it still applies, update it; if not, ignore this.".to_string();
            if !tasks.is_empty() {
                let lines: Vec<String> = tasks.iter().map(|t| format!("- [{}] {}", t.status, t.subject)).collect();
                text.push_str(&format!(" Current tasks:\n{}", lines.join("\n")));
            }
            ("task_reminder", text)
        }
        TurnEvent::PlanMode { entered: true } => (
            "plan_mode",
            "Plan mode is on: read and research only, then propose the plan. Nothing changes until the owner approves it.".to_string(),
        ),
        TurnEvent::PlanMode { entered: false } => (
            "plan_mode",
            "Plan mode is off: the owner approved the plan. Carry it out.".to_string(),
        ),
        TurnEvent::GoalSet(condition) => (
            "goal_set",
            format!("Agreed goal: {}. Work continues until a separate check confirms it is met.", non_empty(condition)?),
        ),
        TurnEvent::GoalCleared => ("goal_cleared", "The agreed goal was cleared.".to_string()),
        TurnEvent::GoalCheck { reason, condition } => (
            "goal_check",
            format!("The agreed goal isn't met yet: {}. Keep working toward: {}.", reason.trim(), condition.trim()),
        ),
        TurnEvent::Usage(t) => ("usage", threshold_text(t)),
        TurnEvent::ToolsAvailable(d) => {
            let text = non_empty(&render_listing(d))?;
            let added = d.added.iter().map(|n| (n.clone(), String::new())).collect();
            return Some(listing_row("tools_available", text, &added, &d.removed));
        }
        TurnEvent::SkillListing(d) => return d.attachment("skill_listing", &SKILL_WORDS),
        TurnEvent::HelperTypes(d) => return d.attachment("helper_types", &HELPER_WORDS),
        TurnEvent::BackgroundUpdate(text) => ("background_update", non_empty(text)?),
        TurnEvent::CutoffResume => (
            "cutoff_resume",
            "Your last reply hit the output limit. Continue exactly where it stopped.".to_string(),
        ),
        TurnEvent::WorkflowContract(text) => ("workflow_contract", non_empty(text)?),
        TurnEvent::AppHook { text, .. } => ("app_hook", non_empty(text)?),
    };
    Some(Attachment {
        kind,
        text,
        data: serde_json::Map::new(),
    })
}

fn non_empty(text: &str) -> Option<String> {
    let t = text.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn threshold_text(t: &Threshold) -> String {
    match *t {
        Threshold::Context { percent_full } => format!("Context {percent_full}% full."),
        Threshold::Spend {
            spent_microcents,
            cap_microcents,
        } => format!(
            "Spend ${:.2} of ${:.2}.",
            spent_microcents as f64 / MICROCENTS_PER_DOLLAR,
            cap_microcents as f64 / MICROCENTS_PER_DOLLAR
        ),
    }
}

// ── Listings: deferred tools, skills, helper types ──────────────────────
//
// A listing row stores what it announced (`added`: name → line, `removed`:
// names) next to its kind, so the set the conversation was last told is
// folded back from its rows (Claude Code's delta attachments). A listing is
// written only when that set differs from the current one; after a
// checkpoint the fold starts empty and the next step lists the set whole.

/// A listing: name → line (empty for deferred tools, which list names only).
pub type Listing = BTreeMap<String, String>;

/// A change in a listing whose entries carry one line each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinedDelta {
    /// New entries and entries whose line changed.
    pub added: Listing,
    pub removed: BTreeSet<String>,
}

impl LinedDelta {
    /// The change from `announced` to `now`, or None when nothing changed.
    pub fn between(announced: &Listing, now: &Listing) -> Option<LinedDelta> {
        let added: Listing = now
            .iter()
            .filter(|(n, line)| announced.get(*n) != Some(*line))
            .map(|(n, line)| (n.clone(), line.clone()))
            .collect();
        let removed: BTreeSet<String> = announced.keys().filter(|n| !now.contains_key(*n)).cloned().collect();
        (!added.is_empty() || !removed.is_empty()).then_some(LinedDelta { added, removed })
    }

    /// `- name: line` under each header.
    fn attachment(&self, kind: &'static str, words: &ListingWords) -> Option<Attachment> {
        let mut sections = Vec::new();
        if !self.added.is_empty() {
            let lines: Vec<String> = self
                .added
                .iter()
                .map(|(n, line)| if line.is_empty() { format!("- {n}") } else { format!("- {n}: {line}") })
                .collect();
            sections.push(format!("{}\n{}", words.available, lines.join("\n")));
        }
        if !self.removed.is_empty() {
            let lines: Vec<String> = self.removed.iter().map(|n| format!("- {n}")).collect();
            sections.push(format!("{}\n{}", words.removed, lines.join("\n")));
        }
        if sections.is_empty() {
            return None;
        }
        Some(listing_row(kind, sections.join("\n\n"), &self.added, &self.removed))
    }
}

struct ListingWords {
    available: &'static str,
    removed: &'static str,
}

const SKILL_WORDS: ListingWords = ListingWords {
    available: "These skills are available through use_skill:",
    removed: "These skills are no longer available:",
};

const HELPER_WORDS: ListingWords = ListingWords {
    available: "These helper types are available to delegate:",
    removed: "These helper types are no longer available:",
};

fn listing_row(kind: &'static str, text: String, added: &Listing, removed: &BTreeSet<String>) -> Attachment {
    let added: serde_json::Map<String, serde_json::Value> =
        added.iter().map(|(n, l)| (n.clone(), serde_json::Value::String(l.clone()))).collect();
    Attachment {
        kind,
        text,
        data: serde_json::Map::from_iter([
            ("added".to_string(), serde_json::Value::Object(added)),
            ("removed".to_string(), serde_json::json!(removed)),
        ]),
    }
}

/// The listing of `kind` the conversation was last told: its listing rows
/// since the boundary, folded in order.
pub fn announced(kind: &str, history: &[ChatMessage]) -> Listing {
    let mut listing = Listing::new();
    for fields in history.iter().filter_map(attachment_fields) {
        if fields.get("kind").and_then(|k| k.as_str()) != Some(kind) {
            continue;
        }
        if let Some(removed) = fields.get("removed").and_then(|r| r.as_array()) {
            for name in removed.iter().filter_map(|n| n.as_str()) {
                listing.remove(name);
            }
        }
        if let Some(added) = fields.get("added").and_then(|a| a.as_object()) {
            for (name, line) in added {
                listing.insert(name.clone(), line.as_str().unwrap_or_default().to_string());
            }
        }
    }
    listing
}

// ── The task reminder ───────────────────────────────────────────────────

/// Steps without a task-tool call before the task reminder, and the least
/// number of steps between two of them (Claude Code's counts).
pub const TASK_REMINDER_STEPS: usize = 10;

/// The tools whose use resets the count: creating or updating a task.
pub const TASK_TOOLS: &[&str] = &["create_task", "update_task"];

/// Whether the task reminder is due, counted from the conversation: at least
/// `TASK_REMINDER_STEPS` model replies since a task tool was last called and
/// since the last task reminder. The caller asks only when the task tools
/// are on the turn's surface.
pub fn task_reminder_due(history: &[ChatMessage]) -> bool {
    let mut since_task_use = None;
    let mut since_reminder = None;
    let mut replies = 0;
    for m in history.iter().rev() {
        if m.role == "assistant" {
            if since_task_use.is_none() && calls_task_tool(m) {
                since_task_use = Some(replies);
            }
            replies += 1;
        } else if since_reminder.is_none()
            && attachment_fields(m).is_some_and(|f| f.get("kind").and_then(|k| k.as_str()) == Some("task_reminder"))
        {
            since_reminder = Some(replies);
        }
        if since_task_use.is_some() && since_reminder.is_some() {
            break;
        }
    }
    since_task_use.unwrap_or(replies) >= TASK_REMINDER_STEPS
        && since_reminder.unwrap_or(replies) >= TASK_REMINDER_STEPS
}

fn calls_task_tool(m: &ChatMessage) -> bool {
    m.tool_calls
        .as_deref()
        .and_then(|tc| serde_json::from_str::<Vec<serde_json::Value>>(tc).ok())
        .is_some_and(|calls| {
            calls
                .iter()
                .any(|c| c.get("name").and_then(|n| n.as_str()).is_some_and(|n| TASK_TOOLS.contains(&n)))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scored(key: &str, value: &str) -> crate::memory::ScoredMemory {
        crate::memory::ScoredMemory {
            memory: db::models::Memory {
                id: 1,
                namespace: "tacit".into(),
                key: key.into(),
                value: value.into(),
                tags: None,
                metadata: None,
                created_at: None,
                updated_at: None,
                accessed_at: None,
                access_count: None,
                user_id: "owner".into(),
            },
            score: 1.0,
        }
    }

    fn row(role: &str, content: &str, tool_calls: Option<&str>, metadata: Option<serde_json::Value>) -> ChatMessage {
        ChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            chat_id: "c".into(),
            role: role.into(),
            content: content.into(),
            metadata: metadata.map(|m| m.to_string()),
            created_at: 0,
            day_marker: None,
            tool_calls: tool_calls.map(str::to_string),
            tool_results: None,
            token_estimate: None,
            html: None,
        }
    }

    /// The row the writer stores for an attachment.
    fn stored(a: &Attachment) -> ChatMessage {
        row("user", &crate::harness::reminders::wrap(&a.text), None, Some(a.metadata()))
    }

    fn every_event() -> Vec<TurnEvent> {
        let lined = LinedDelta::between(&Listing::new(), &Listing::from([("x".to_string(), "y".to_string())])).unwrap();
        vec![
            TurnEvent::DateChanged(chrono::NaiveDate::from_ymd_opt(2026, 9, 24).unwrap()),
            TurnEvent::RunBriefing("team: Ann".into()),
            TurnEvent::RestrictedRun("outside origin".into()),
            TurnEvent::RelevantMemories(vec![scored("k", "v")]),
            TurnEvent::FilesChanged(vec!["a.rs changed".into()]),
            TurnEvent::Diagnostics(vec!["a.rs:1 error".into()]),
            TurnEvent::TasksIdle(Vec::new()),
            TurnEvent::PlanMode { entered: true },
            TurnEvent::GoalSet("all tests pass".into()),
            TurnEvent::GoalCleared,
            TurnEvent::GoalCheck {
                reason: "\"2 failing\"".into(),
                condition: "all tests pass".into(),
            },
            TurnEvent::Usage(Threshold::Context { percent_full: 82 }),
            TurnEvent::ToolsAvailable(ToolsDelta::all(["vm".to_string()].into())),
            TurnEvent::SkillListing(lined.clone()),
            TurnEvent::HelperTypes(lined),
            TurnEvent::BackgroundUpdate("the export finished".into()),
            TurnEvent::CutoffResume,
            TurnEvent::WorkflowContract("call publish once".into()),
            TurnEvent::AppHook {
                label: "app".into(),
                text: "the invoice is due".into(),
            },
        ]
    }

    #[test]
    fn every_event_has_one_named_row() {
        let mut kinds: Vec<&str> = every_event()
            .iter()
            .map(|e| attachment_for(e).unwrap_or_else(|| panic!("{e:?} speaks")).kind)
            .collect();
        kinds.sort();
        kinds.dedup();
        let mut names = NAMES.to_vec();
        names.sort();
        assert_eq!(kinds, names, "the table and NAMES agree");
    }

    #[test]
    fn empty_events_say_nothing() {
        for e in [
            TurnEvent::RunBriefing("  ".into()),
            TurnEvent::RelevantMemories(Vec::new()),
            TurnEvent::FilesChanged(Vec::new()),
            TurnEvent::Diagnostics(vec![String::new()]),
            TurnEvent::BackgroundUpdate(String::new()),
            TurnEvent::GoalSet(" ".into()),
            TurnEvent::AppHook {
                label: "app".into(),
                text: String::new(),
            },
        ] {
            assert!(attachment_for(&e).is_none(), "{e:?}");
        }
    }

    #[test]
    fn facts_state_the_fact() {
        let text = |e: TurnEvent| attachment_for(&e).unwrap().text;
        assert_eq!(
            text(TurnEvent::DateChanged(chrono::NaiveDate::from_ymd_opt(2026, 9, 24).unwrap())),
            "The date is now Thursday, September 24, 2026."
        );
        assert_eq!(
            text(TurnEvent::Usage(Threshold::Spend {
                spent_microcents: 410_000_000,
                cap_microcents: 500_000_000,
            })),
            "Spend $4.10 of $5.00."
        );
        assert_eq!(text(TurnEvent::Usage(Threshold::Context { percent_full: 82 })), "Context 82% full.");
        assert_eq!(
            text(TurnEvent::GoalCheck {
                reason: "the log shows \"2 failed\"".into(),
                condition: "all tests pass".into(),
            }),
            "The agreed goal isn't met yet: the log shows \"2 failed\". Keep working toward: all tests pass."
        );
    }

    #[test]
    fn recall_surfaces_at_most_five() {
        let found: Vec<_> = (0..8).map(|i| scored(&format!("k{i}"), "v")).collect();
        let text = attachment_for(&TurnEvent::RelevantMemories(found)).unwrap().text;
        assert!(text.starts_with("Memories that may apply:"));
        assert_eq!(text.lines().filter(|l| l.starts_with("- ")).count(), 5);
    }

    #[test]
    fn task_reminder_after_10_idle_steps_at_most_every_10() {
        let task_call = r#"[{"id":"t1","name":"update_task","input":{}}]"#;
        let other_call = r#"[{"id":"o1","name":"read_file","input":{}}]"#;
        let mut history = vec![row("user", "Ship it", None, None), row("assistant", "", Some(task_call), None)];
        let reply = || row("assistant", "", Some(other_call), None);

        for _ in 0..9 {
            history.push(reply());
        }
        assert!(!task_reminder_due(&history), "9 replies since the task tool");
        history.push(reply());
        assert!(task_reminder_due(&history), "10 replies since the task tool");

        let reminder = attachment_for(&TurnEvent::TasksIdle(vec![WorkTaskLine {
            subject: "send the invoice".into(),
            status: "in_progress".into(),
        }]))
        .unwrap();
        assert_eq!(reminder.kind, "task_reminder");
        assert!(reminder.text.ends_with("Current tasks:\n- [in_progress] send the invoice"), "{}", reminder.text);
        history.push(stored(&reminder));
        assert!(!task_reminder_due(&history), "just reminded");
        for _ in 0..9 {
            history.push(reply());
        }
        assert!(!task_reminder_due(&history), "9 replies since the reminder");
        history.push(reply());
        assert!(task_reminder_due(&history), "10 replies since the reminder");

        history.push(row("assistant", "", Some(task_call), None));
        assert!(!task_reminder_due(&history), "the task tool was used");

        let fresh: Vec<ChatMessage> = (0..10).map(|_| reply()).collect();
        assert!(task_reminder_due(&fresh), "never used and never reminded");
    }

    #[test]
    fn listings_written_only_when_the_set_changes() {
        let set = |names: &[&str]| -> BTreeSet<String> { names.iter().map(|n| n.to_string()).collect() };
        let mut history = vec![row("user", "hello", None, None)];
        // What a step does: compare the listed set with what the
        // conversation was told, and write a row only on a change.
        let step = |history: &mut Vec<ChatMessage>, now: &BTreeSet<String>| -> Option<Attachment> {
            let told: BTreeSet<String> = announced("tools_available", history).into_keys().collect();
            let a = attachment_for(&TurnEvent::ToolsAvailable(ToolsDelta::between(&told, now)?))?;
            history.push(stored(&a));
            Some(a)
        };

        let first = step(&mut history, &set(&["mail_send", "web_fetch"])).expect("first listing");
        assert!(first.text.ends_with("One name per line:\nmail_send\nweb_fetch"), "{}", first.text);
        assert!(step(&mut history, &set(&["mail_send", "web_fetch"])).is_none(), "same set, no row");

        let changed = step(&mut history, &set(&["mail_send", "calendar_add"])).expect("a change");
        assert!(changed.text.contains("One name per line:\ncalendar_add"), "{}", changed.text);
        assert!(changed.text.ends_with("no longer available:\nweb_fetch"), "{}", changed.text);
        assert!(!changed.text.contains("mail_send"), "only the change is written");
        assert!(step(&mut history, &set(&["calendar_add", "mail_send"])).is_none());
        assert_eq!(history.len(), 3, "two listing rows in four steps");

        // Skills: a changed line is a change; the same lines are not.
        let skills = |pairs: &[(&str, &str)]| -> Listing {
            pairs.iter().map(|(n, l)| (n.to_string(), l.to_string())).collect()
        };
        let mut skill_step = |now: &Listing| -> Option<Attachment> {
            let a = attachment_for(&TurnEvent::SkillListing(LinedDelta::between(&announced("skill_listing", &history), now)?))?;
            history.push(stored(&a));
            Some(a)
        };
        assert!(skill_step(&skills(&[("invoice", "Draft an invoice")])).is_some());
        assert!(skill_step(&skills(&[("invoice", "Draft an invoice")])).is_none());
        let edited = skill_step(&skills(&[("invoice", "Draft and send an invoice")])).expect("line changed");
        assert_eq!(edited.text, format!("{}\n- invoice: Draft and send an invoice", SKILL_WORDS.available));
        assert_eq!(
            announced("tools_available", &history).into_keys().collect::<Vec<_>>(),
            ["calendar_add", "mail_send"],
            "each kind folds its own rows"
        );

        // After a checkpoint nothing was announced: the next step lists the set whole.
        assert!(announced("tools_available", &[]).is_empty());
    }

    #[test]
    fn skill_and_helper_listings_carry_their_lines() {
        let now = Listing::from([("invoice".to_string(), "Draft an invoice".to_string())]);
        let delta = LinedDelta::between(&Listing::new(), &now).unwrap();
        let skills = attachment_for(&TurnEvent::SkillListing(delta)).unwrap();
        assert_eq!(skills.text, format!("{}\n- invoice: Draft an invoice", SKILL_WORDS.available));
        let gone = LinedDelta::between(&now, &Listing::new()).unwrap();
        let helpers = attachment_for(&TurnEvent::HelperTypes(gone)).unwrap();
        assert_eq!(helpers.text, format!("{}\n- invoice", HELPER_WORDS.removed));
    }
}
