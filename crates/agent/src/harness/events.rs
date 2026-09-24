//! Turn events and THE table that turns each into an attachment. Every row
//! states a fact at the moment it became true, except the task reminder,
//! which fires on Claude Code's step counts. A new attachment is one row
//! here plus its producer.

use std::collections::BTreeMap;

use db::models::ChatMessage;

use super::reminders::{Attachment, attachment_fields};

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
    /// The deferred-tool set changed. A listing line is the definition's
    /// fingerprint and is never shown.
    ToolsAvailable(ListingDelta),
    /// The skill set changed; a line is the skill's one-line description.
    SkillListing(ListingDelta),
    /// The helper types changed; a line is the type's use and tool set.
    HelperTypes(ListingDelta),
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
        TurnEvent::ToolsAvailable(d) => return d.attachment("tools_available", &TOOL_WORDS, false),
        TurnEvent::SkillListing(d) => return d.attachment("skill_listing", &SKILL_WORDS, true),
        TurnEvent::HelperTypes(d) => return d.attachment("helper_types", &HELPER_WORDS, true),
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

/// A listing: name → line.
pub type Listing = BTreeMap<String, String>;

/// How a listing changed since it was last announced. `now` is stored on
/// the row, so the next comparison reads it back from the conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingDelta {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub removed: Vec<String>,
    pub now: Listing,
}

impl ListingDelta {
    /// The change from `before` to `now`, or None when the set is the same.
    pub fn between(before: &Listing, now: &Listing) -> Option<ListingDelta> {
        let added: Vec<String> = now.keys().filter(|n| !before.contains_key(*n)).cloned().collect();
        let updated: Vec<String> = now
            .iter()
            .filter(|(n, line)| before.get(*n).is_some_and(|old| old != *line))
            .map(|(n, _)| n.clone())
            .collect();
        let removed: Vec<String> = before.keys().filter(|n| !now.contains_key(*n)).cloned().collect();
        if added.is_empty() && updated.is_empty() && removed.is_empty() {
            return None;
        }
        Some(ListingDelta {
            added,
            updated,
            removed,
            now: now.clone(),
        })
    }

    /// The listing row: names only for tools (grouped past 30), `- name:
    /// line` for skills and helper types.
    fn attachment(&self, kind: &'static str, words: &ListingWords, show_lines: bool) -> Option<Attachment> {
        let render = |names: &[String], with_line: bool| -> Vec<String> {
            if !show_lines {
                return group_names(names);
            }
            names
                .iter()
                .map(|n| match self.now.get(n) {
                    Some(line) if with_line && !line.is_empty() => format!("- {n}: {line}"),
                    _ => format!("- {n}"),
                })
                .collect()
        };
        let sections: Vec<String> = [
            (words.added, &self.added, true),
            (words.updated, &self.updated, true),
            (words.removed, &self.removed, false),
        ]
        .into_iter()
        .filter(|(_, names, _)| !names.is_empty())
        .map(|(header, names, with_line)| format!("{header}\n{}", render(names, with_line).join("\n")))
        .collect();
        if sections.is_empty() {
            return None;
        }
        let listing = self
            .now
            .iter()
            .map(|(n, l)| (n.clone(), serde_json::Value::String(l.clone())))
            .collect();
        Some(Attachment {
            kind,
            text: sections.join("\n\n"),
            data: serde_json::Map::from_iter([("listing".to_string(), serde_json::Value::Object(listing))]),
        })
    }
}

struct ListingWords {
    added: &'static str,
    updated: &'static str,
    removed: &'static str,
}

const TOOL_WORDS: ListingWords = ListingWords {
    added: "These deferred tools are now available through find_tools. Their definitions aren't loaded: load them with find_tools(\"select:<name>[,<name>…]\") before calling. One name per line:",
    updated: "These deferred tools have updated definitions. Load them again with find_tools before calling:",
    removed: "These deferred tools are no longer available:",
};

const SKILL_WORDS: ListingWords = ListingWords {
    added: "These skills are now available through use_skill:",
    updated: "These skills have updated instructions:",
    removed: "These skills are no longer available:",
};

const HELPER_WORDS: ListingWords = ListingWords {
    added: "These helper types are now available to delegate:",
    updated: "These helper types have changed:",
    removed: "These helper types are no longer available:",
};

/// Past this many names, the external families group by server or app.
const GROUP_PAST: usize = 30;

/// Tool names in a listing, one per line. Past 30 names, `mcp__<server>__*`
/// and `app__<app>__*` names group to one line each with their count.
fn group_names(names: &[String]) -> Vec<String> {
    if names.len() <= GROUP_PAST {
        return names.to_vec();
    }
    let family = |name: &str| -> Option<String> {
        let rest = name.strip_prefix("mcp__").or_else(|| name.strip_prefix("app__"))?;
        let (owner, _) = rest.split_once("__")?;
        Some(format!("{}{owner}__*", &name[..name.len() - rest.len()]))
    };
    let mut lines = Vec::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for name in names {
        match family(name) {
            Some(group) => *counts.entry(group).or_default() += 1,
            None => lines.push(name.clone()),
        }
    }
    lines.extend(counts.into_iter().map(|(group, n)| format!("{group} ({n})")));
    lines
}

/// The listing of `kind` the conversation last announced: the set stored on
/// its latest listing row since the boundary, empty when there is none (a
/// fresh conversation, or one just checkpointed, gets the full listing).
pub fn announced(kind: &str, history: &[ChatMessage]) -> Listing {
    history
        .iter()
        .rev()
        .filter_map(attachment_fields)
        .find(|f| f.get("kind").and_then(|k| k.as_str()) == Some(kind))
        .and_then(|f| f.get("listing").and_then(|l| l.as_object()).cloned())
        .map(|l| {
            l.into_iter()
                .map(|(n, line)| (n, line.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
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
        let delta = ListingDelta::between(&Listing::new(), &Listing::from([("x".to_string(), "y".to_string())])).unwrap();
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
            TurnEvent::ToolsAvailable(delta.clone()),
            TurnEvent::SkillListing(delta.clone()),
            TurnEvent::HelperTypes(delta),
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
        let tools = |pairs: &[(&str, &str)]| -> Listing {
            pairs.iter().map(|(n, l)| (n.to_string(), l.to_string())).collect()
        };
        let mut history = vec![row("user", "hello", None, None)];
        let step = |history: &mut Vec<ChatMessage>, now: &Listing| -> Option<Attachment> {
            let delta = ListingDelta::between(&announced("tools_available", history), now)?;
            let a = attachment_for(&TurnEvent::ToolsAvailable(delta))?;
            history.push(stored(&a));
            Some(a)
        };

        let first = step(&mut history, &tools(&[("mail_send", "v1"), ("web_fetch", "v1")])).expect("first listing");
        assert!(first.text.ends_with("One name per line:\nmail_send\nweb_fetch"), "{}", first.text);
        assert!(!first.text.contains("v1"), "tool lines are names only");

        assert!(step(&mut history, &tools(&[("mail_send", "v1"), ("web_fetch", "v1")])).is_none(), "same set, no row");

        let changed = step(&mut history, &tools(&[("mail_send", "v2"), ("calendar_add", "v1")])).expect("a change");
        assert_eq!(
            changed.text,
            format!(
                "{}\ncalendar_add\n\n{}\nmail_send\n\n{}\nweb_fetch",
                TOOL_WORDS.added, TOOL_WORDS.updated, TOOL_WORDS.removed
            )
        );
        assert!(step(&mut history, &tools(&[("mail_send", "v2"), ("calendar_add", "v1")])).is_none());

        // Another listing kind is announced separately.
        assert!(announced("skill_listing", &history).is_empty());
        // After a checkpoint the announced set is gone and the listing is re-sent whole.
        assert!(ListingDelta::between(&announced("tools_available", &[]), &tools(&[("mail_send", "v2")])).is_some());
    }

    #[test]
    fn skill_and_helper_listings_carry_their_lines() {
        let now = Listing::from([("invoice".to_string(), "Draft an invoice".to_string())]);
        let delta = ListingDelta::between(&Listing::new(), &now).unwrap();
        let skills = attachment_for(&TurnEvent::SkillListing(delta.clone())).unwrap();
        assert_eq!(skills.text, format!("{}\n- invoice: Draft an invoice", SKILL_WORDS.added));
        let gone = ListingDelta::between(&now, &Listing::new()).unwrap();
        let helpers = attachment_for(&TurnEvent::HelperTypes(gone)).unwrap();
        assert_eq!(helpers.text, format!("{}\n- invoice", HELPER_WORDS.removed));
    }

    #[test]
    fn past_thirty_names_external_families_group() {
        let mut names: Vec<String> = (0..25).map(|i| format!("mcp__crm__op{i:02}")).collect();
        names.extend((0..6).map(|i| format!("app__books__op{i}")));
        names.push("mail_send".into());
        assert_eq!(group_names(&names), vec!["mail_send", "app__books__* (6)", "mcp__crm__* (25)"]);
        let few: Vec<String> = (0..3).map(|i| format!("mcp__crm__op{i}")).collect();
        assert_eq!(group_names(&few), few);
    }
}
