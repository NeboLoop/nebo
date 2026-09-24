//! Turn events and THE table that turns each into a reminder. A tuned piece
//! of steering comes back by adding one row here plus its producer.

use super::reminders::Kind;

/// Something that happened during a turn that the model hears about on its
/// next call. WP1.2 adds `CapabilityMissing(types::OwnerNeed)` once #237 is
/// on main.
#[derive(Debug, Clone)]
pub enum TurnEvent {
    MessageTime(chrono::DateTime<chrono::Local>),
    /// Team roster, @mention, room briefing.
    RunBriefing(String),
    RestrictedRun(String),
    RelevantMemories(Vec<crate::memory::ScoredMemory>),
    /// The read ledger's change notes.
    FilesChanged(Vec<String>),
    Diagnostics(Vec<String>),
    /// The same call with the same arguments, earlier in this turn.
    RepeatedCall {
        tool: String,
    },
    Threshold(Threshold),
    CutoffResume,
    LostToolCalls,
    /// Deferred tools became callable.
    ToolsLoaded(Vec<String>),
    /// The deferred tools listed by name changed (tools doc §4.1).
    ToolsAvailable(super::tool_surface::ListingDelta),
    /// The `steering.generate` hook's text.
    AppDirective {
        label: String,
        text: String,
    },
    EndCheckContinue {
        check: &'static str,
        text: String,
    },
    /// The work tasks went untouched; see `task_reminder_due`.
    TasksUntouched {
        tasks: Vec<String>,
    },
    /// An agreed goal was set; said once, never re-rendered.
    GoalSet {
        condition: String,
    },
    GoalCleared,
    /// A helper was launched; its completion arrives as a notification.
    HelperLaunched {
        task_id: String,
        description: String,
    },
    /// A background result arrived.
    BackgroundResult(String),
    /// The owner's presence changed.
    Presence(String),
}

/// Steps without a task-tool call before the task reminder, and the least
/// number of steps between two of them.
pub const TASK_REMINDER_STEPS: u32 = 10;

/// Whether the work-task reminder is due: `TASK_REMINDER_STEPS` steps with no
/// task-tool use, and as many since the last one (`None` = never sent).
pub fn task_reminder_due(steps_since_task_use: u32, steps_since_reminder: Option<u32>) -> bool {
    steps_since_task_use >= TASK_REMINDER_STEPS
        && steps_since_reminder.is_none_or(|s| s >= TASK_REMINDER_STEPS)
}

/// A limit the turn is nearing: the number only, never an instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Threshold {
    Context {
        percent_full: u8,
    },
    Spend {
        spent_microcents: i64,
        cap_microcents: i64,
    },
    Quota {
        percent_used: u8,
    },
    Rate {
        retry_after_secs: u64,
    },
}

/// The steering names in the table — what `NEBO_STEERING` can address.
pub const STEERING_NAMES: &[&str] = &["app_steering"];

/// Most memories one recall surfaces.
const MAX_RECALLED: usize = 5;

const MICROCENTS_PER_DOLLAR: f64 = 100_000_000.0;

/// THE table: an event's reminder name, kind and text, or None when the
/// event has nothing to say.
pub fn reminder_for(e: &TurnEvent) -> Option<(&'static str, Kind, String)> {
    let fact = |name: &'static str, text: String| Some((name, Kind::Fact, text));
    match e {
        TurnEvent::MessageTime(ts) => fact(
            "message_time",
            format!(
                "Message sent at {}.",
                ts.format("%A, %B %-d, %Y %-I:%M %p %Z")
            ),
        ),
        TurnEvent::RunBriefing(text) => non_empty(text).and_then(|t| fact("run_briefing", t)),
        TurnEvent::RestrictedRun(text) => non_empty(text).and_then(|t| fact("restricted_run", t)),
        TurnEvent::RelevantMemories(found) => {
            if found.is_empty() {
                return None;
            }
            let lines: Vec<String> = found
                .iter()
                .take(MAX_RECALLED)
                .map(|m| format!("- {}: {}", m.memory.key, m.memory.value))
                .collect();
            fact(
                "relevant_memories",
                format!("Memories that may apply:\n{}", lines.join("\n")),
            )
        }
        TurnEvent::FilesChanged(notes) | TurnEvent::Diagnostics(notes) => {
            if notes.is_empty() {
                return None;
            }
            fact("files_changed", notes.join("\n"))
        }
        TurnEvent::RepeatedCall { tool } => fact(
            "repeated_call",
            format!("You made this exact {tool} call earlier in this turn; its result is above."),
        ),
        TurnEvent::Threshold(t) => fact("threshold", threshold_text(t)),
        TurnEvent::CutoffResume => fact(
            "cutoff_resume",
            "Your last reply hit the output limit. Continue where it stopped.".to_string(),
        ),
        TurnEvent::LostToolCalls => fact(
            "lost_tool_calls",
            "Your last reply ended to call tools but none arrived. Make the calls.".to_string(),
        ),
        TurnEvent::ToolsLoaded(names) => {
            if names.is_empty() {
                return None;
            }
            fact(
                "tools_loaded",
                format!("Now callable: {}.", names.join(", ")),
            )
        }
        TurnEvent::ToolsAvailable(delta) => {
            non_empty(&super::tool_surface::render_listing(delta)).and_then(|t| fact("tools_available", t))
        }
        TurnEvent::AppDirective { text, .. } => {
            non_empty(text).map(|t| ("app_steering", Kind::Steering, t))
        }
        TurnEvent::EndCheckContinue { text, .. } => fact("goal_check", text.clone()),
        TurnEvent::TasksUntouched { tasks } => {
            if tasks.is_empty() {
                return None;
            }
            let list: Vec<String> = tasks.iter().map(|t| format!("- {t}")).collect();
            fact(
                "work_tasks",
                format!(
                    "The work tasks haven't been updated in a while. If they still apply, update them; if not, ignore this.\n{}",
                    list.join("\n")
                ),
            )
        }
        TurnEvent::GoalSet { condition } => {
            fact("goal_set", format!("Agreed goal set: {condition}"))
        }
        TurnEvent::GoalCleared => fact("goal_cleared", "The agreed goal was cleared.".to_string()),
        TurnEvent::HelperLaunched {
            task_id,
            description,
        } => fact(
            "helper_launched",
            format!(
                "Helper {task_id} \"{description}\" is running in the background. Its result arrives as a notification."
            ),
        ),
        TurnEvent::BackgroundResult(text) => {
            non_empty(text).and_then(|t| fact("background_result", t))
        }
        TurnEvent::Presence(text) => non_empty(text).and_then(|t| fact("presence", t)),
    }
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
        Threshold::Quota { percent_used } => format!("Quota {percent_used}% used."),
        Threshold::Rate { retry_after_secs } => {
            format!("Rate limited; the next call can go in {retry_after_secs}s.")
        }
    }
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

    #[test]
    fn every_steering_row_is_a_known_name_and_facts_are_not() {
        let directive = TurnEvent::AppDirective {
            label: "app".into(),
            text: "focus on the invoice".into(),
        };
        let (name, kind, text) = reminder_for(&directive).expect("a directive speaks");
        assert_eq!((name, kind), ("app_steering", Kind::Steering));
        assert!(STEERING_NAMES.contains(&name));
        assert_eq!(text, "focus on the invoice");

        let facts = [
            TurnEvent::RunBriefing("team: Ann".into()),
            TurnEvent::RestrictedRun("no tools".into()),
            TurnEvent::RelevantMemories(vec![scored("k", "v")]),
            TurnEvent::FilesChanged(vec!["a.rs changed".into()]),
            TurnEvent::Diagnostics(vec!["a.rs:1 error".into()]),
            TurnEvent::RepeatedCall { tool: "os".into() },
            TurnEvent::Threshold(Threshold::Context { percent_full: 82 }),
            TurnEvent::CutoffResume,
            TurnEvent::LostToolCalls,
            TurnEvent::ToolsLoaded(vec!["mail".into()]),
            TurnEvent::ToolsAvailable(super::super::tool_surface::ListingDelta::all(["vm".to_string()].into())),
            TurnEvent::EndCheckContinue {
                check: "goal",
                text: "not met".into(),
            },
            TurnEvent::MessageTime(chrono::Local::now()),
            TurnEvent::TasksUntouched {
                tasks: vec!["send the invoice".into()],
            },
            TurnEvent::GoalSet {
                condition: "all tests pass".into(),
            },
            TurnEvent::GoalCleared,
            TurnEvent::HelperLaunched {
                task_id: "h1".into(),
                description: "read the logs".into(),
            },
            TurnEvent::BackgroundResult("the export finished".into()),
            TurnEvent::Presence("the owner is away".into()),
        ];
        for e in &facts {
            let (name, kind, _) = reminder_for(e).unwrap_or_else(|| panic!("{e:?} speaks"));
            assert_eq!(kind, Kind::Fact, "{name}");
            assert!(!STEERING_NAMES.contains(&name), "{name}");
        }
    }

    #[test]
    fn empty_events_say_nothing() {
        for e in [
            TurnEvent::RunBriefing("  ".into()),
            TurnEvent::RelevantMemories(Vec::new()),
            TurnEvent::FilesChanged(Vec::new()),
            TurnEvent::ToolsLoaded(Vec::new()),
            TurnEvent::TasksUntouched { tasks: Vec::new() },
            TurnEvent::AppDirective {
                label: "app".into(),
                text: String::new(),
            },
        ] {
            assert!(reminder_for(&e).is_none(), "{e:?}");
        }
    }

    #[test]
    fn recall_surfaces_at_most_five() {
        let found: Vec<_> = (0..8).map(|i| scored(&format!("k{i}"), "v")).collect();
        let (_, _, text) = reminder_for(&TurnEvent::RelevantMemories(found)).unwrap();
        assert!(text.starts_with("Memories that may apply:"));
        assert_eq!(text.lines().filter(|l| l.starts_with("- ")).count(), 5);
    }

    #[test]
    fn task_reminder_after_ten_quiet_steps_at_most_every_ten() {
        assert!(!task_reminder_due(9, None));
        assert!(task_reminder_due(10, None));
        assert!(!task_reminder_due(15, Some(9)), "sent 9 steps ago");
        assert!(task_reminder_due(15, Some(10)));
        let (name, _, text) = reminder_for(&TurnEvent::TasksUntouched {
            tasks: vec!["a".into(), "b".into()],
        })
        .unwrap();
        assert_eq!(name, "work_tasks");
        assert!(text.ends_with("- a\n- b"), "{text}");
    }

    #[test]
    fn thresholds_state_the_number() {
        let spend = Threshold::Spend {
            spent_microcents: 410_000_000,
            cap_microcents: 500_000_000,
        };
        assert_eq!(threshold_text(&spend), "Spend $4.10 of $5.00.");
        assert_eq!(
            threshold_text(&Threshold::Context { percent_full: 82 }),
            "Context 82% full."
        );
    }

    #[test]
    fn a_listing_change_is_one_names_only_fact_and_no_change_says_nothing() {
        use super::super::tool_surface::ListingDelta;
        let delta = ListingDelta::all(["vm".to_string()].into());
        let (name, kind, text) = reminder_for(&TurnEvent::ToolsAvailable(delta)).unwrap();
        assert_eq!((name, kind), ("tools_available", Kind::Fact));
        assert!(text.ends_with(":\nvm"), "{text}");
        assert!(reminder_for(&TurnEvent::ToolsAvailable(ListingDelta::default())).is_none());
    }
}
