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
    /// The session's facts, whole: the first step of a session, and the
    /// first step after a checkpoint. One row per fact.
    SessionSnapshot(SessionFacts),
    /// The time a turn starts, for the owner (`sections::owner_now`).
    TurnTime(String),
    /// Where the owner's phone is, when the owner shares it with the
    /// employee (`crate::phone_location`); `None` withdraws the readings
    /// the conversation heard, once nothing is shared with it any more.
    PhoneLocation(Option<crate::phone_location::SharedPosition>),
    /// The channel's rules changed, or were never told: the replacement,
    /// whole (`told` says whether there was an earlier version).
    ChannelRulesChanged { rules: String, told: bool },
    /// What the coworker being answered may be told changed: the new limit,
    /// or empty when the limit is lifted.
    CoworkerAccessChanged(String),
    /// Who the turn is for changed (the owner edited the employee): the
    /// replacement, whole.
    IdentityChanged(String),
    /// The workflow activity's instructions changed: the replacement, whole.
    ActivityChanged(String),
    /// Environment fields that changed since the conversation was told.
    EnvironmentChanged(Vec<(String, String)>),
    /// The model or the permission mode changed.
    ModeChanged(ModeFacts),
    /// The employee's memory changed: the replacement, whole.
    EmployeeMemoryChanged(String),
    /// The workspace notes or the employee's own setup changed: the
    /// replacement, whole.
    SessionContextChanged(String),
    /// The installed employees changed; a line is the employee's job (its
    /// description). With [`TurnEvent::TeamsListing`], the roster: who owns
    /// which job (Claude Code's agent listing, `agent_listing_delta`).
    AgentsListing(LinedDelta),
    /// The teams changed: hired, changed or restaffed. A line is what the
    /// team owns, its lead and its members ([`team_line`]).
    TeamsListing(LinedDelta),
    /// The date rolled over mid-session.
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
    /// The connection dropped while the last reply streamed.
    StreamCut,
    /// The last reply had no visible output.
    EmptyReply,
    /// The unmet workflow contract term (workflow mode only).
    WorkflowContract(String),
    /// The `steering.generate` app hook's text.
    AppHook {
        label: String,
        text: String,
    },
    /// After a checkpoint: a file read before it, as it is on disk now.
    RestoredFile {
        path: String,
        content: String,
    },
    /// Skills whose instructions apply, (name, content): after a checkpoint,
    /// the ones loaded before it; for a helper, the ones its parent loaded.
    InvokedSkills(Vec<(String, String)>),
    /// After a checkpoint: work started before it that is still running.
    RunningWork(super::compact::restore::RunningWork),
}

/// What a session knows about itself outside the conversation. Nothing here
/// is in the system prompt: each fact is a row written when it is first
/// told and when it changes, so the prompt and every earlier row stay
/// cached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFacts {
    /// Who the turn is for: the employee, or a helper's role and parent
    /// (`prompt::Identity`).
    pub identity: String,
    /// A workflow activity's instructions; empty on every other turn.
    pub activity: String,
    /// Today in the owner's timezone.
    pub date: chrono::NaiveDate,
    pub timezone: Option<String>,
    /// Platform, shell, working folder, channel, watching; in this order.
    pub environment: Vec<(String, String)>,
    pub mode: ModeFacts,
    /// The identity slice of the employee's memory.
    pub employee_memory: String,
    /// The workspace notes and the employee's own setup.
    pub session_context: String,
    /// How to write for the channel (`sections::channel_rules`); empty for
    /// a channel with none.
    pub channel_rules: String,
    /// The limit on a coworker without shared memory
    /// (`sections::coworker_access`); empty on every other turn.
    pub coworker_access: String,
}

/// The model a session runs on and its permission mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeFacts {
    pub model: String,
    /// By the name the owner sees.
    pub permission_mode: String,
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
    "identity",
    "activity",
    "environment",
    "mode",
    "employee_memory",
    "session_context",
    "channel_rules",
    "coworker_access",
    "time",
    "phone_location",
    "agents_listing",
    "teams_listing",
    "date_changed",
    "run_briefing",
    "restricted_run",
    "relevant_memories",
    "files_changed",
    "diagnostics",
    "task_reminder",
    "plan_mode",
    "goal_set",
    "goal_check",
    "usage",
    "tools_available",
    "skill_listing",
    "helper_types",
    "background_update",
    "cutoff_resume",
    "stream_cut",
    "empty_reply",
    "workflow_contract",
    "app_hook",
    "restored_file",
    "invoked_skills",
    "running_work",
];

/// Most memories one recall surfaces.
pub const MAX_RECALLED: usize = 5;

const MICROCENTS_PER_DOLLAR: f64 = 100_000_000.0;

/// The attachments an event makes: one, none, or for a snapshot one per
/// session fact.
pub fn attachments_for(e: &TurnEvent) -> Vec<Attachment> {
    match e {
        TurnEvent::SessionSnapshot(f) => [
            replacement_row("identity", &f.identity, None),
            replacement_row("activity", &f.activity, None),
            environment_row(f),
            mode_row(&f.mode),
            replacement_row("employee_memory", &f.employee_memory, None),
            replacement_row("session_context", &f.session_context, None),
            replacement_row("channel_rules", &f.channel_rules, None),
            replacement_row("coworker_access", &f.coworker_access, None),
        ]
        .into_iter()
        .flatten()
        .collect(),
        e => attachment_for(e).into_iter().collect(),
    }
}

/// THE table: the attachment an event makes, or None when it has nothing to
/// say. A snapshot makes several (`attachments_for`).
pub fn attachment_for(e: &TurnEvent) -> Option<Attachment> {
    let (kind, text) = match e {
        TurnEvent::SessionSnapshot(_) => return None,
        TurnEvent::TurnTime(now) => ("time", non_empty(now)?),
        TurnEvent::PhoneLocation(position) => {
            let (text, taken) = match position {
                Some(p) => (non_empty(&p.text)?, p.taken.clone()),
                None => (PHONE_LOCATION_WITHDRAWN.to_string(), String::new()),
            };
            return Some(Attachment {
                kind: "phone_location",
                text,
                data: serde_json::Map::from_iter([("taken".to_string(), serde_json::json!(taken))]),
            });
        }
        TurnEvent::ChannelRulesChanged { rules, told } => {
            let lead = told.then_some("The channel's rules have changed; these replace the earlier ones:");
            return replacement_row("channel_rules", rules, lead);
        }
        TurnEvent::CoworkerAccessChanged(limit) => {
            if limit.trim().is_empty() {
                return Some(Attachment {
                    kind: "coworker_access",
                    text: COWORKER_ACCESS_LIFTED.to_string(),
                    data: serde_json::Map::from_iter([("digest".to_string(), serde_json::json!(digest("")))]),
                });
            }
            return replacement_row(
                "coworker_access",
                limit,
                Some("What this coworker may be told has changed; this replaces the earlier note:"),
            );
        }
        TurnEvent::EnvironmentChanged(fields) => {
            if fields.is_empty() {
                return None;
            }
            let lines: Vec<String> = fields.iter().map(|(k, v)| format!("- {k}: {v}")).collect();
            return Some(Attachment {
                kind: "environment",
                text: format!("Environment update:\n{}", lines.join("\n")),
                data: serde_json::Map::from_iter([("fields".to_string(), fields_json(fields))]),
            });
        }
        TurnEvent::IdentityChanged(text) => {
            return replacement_row("identity", text, Some("Who you are has changed; this replaces the earlier version:"));
        }
        TurnEvent::ActivityChanged(text) => {
            return replacement_row("activity", text, Some("The activity's instructions have changed; these replace the earlier ones:"));
        }
        TurnEvent::ModeChanged(m) => return mode_row(m),
        TurnEvent::EmployeeMemoryChanged(text) => {
            return replacement_row("employee_memory", text, Some("Your memory has changed; this replaces the earlier version:"));
        }
        TurnEvent::SessionContextChanged(text) => {
            return replacement_row(
                "session_context",
                text,
                Some("The session context has changed; these values replace the earlier ones:"),
            );
        }
        TurnEvent::AgentsListing(d) => return d.attachment("agents_listing", &AGENT_WORDS),
        TurnEvent::TeamsListing(d) => return d.attachment("teams_listing", &TEAM_WORDS),
        TurnEvent::DateChanged(date) => {
            return Some(Attachment {
                kind: "date_changed",
                text: format!("The date has changed. Today is {}. No need to announce it.", date.format("%A, %B %-d, %Y")),
                data: serde_json::Map::from_iter([("date".to_string(), serde_json::json!(date.to_string()))]),
            });
        }
        TurnEvent::RunBriefing(text) => ("run_briefing", non_empty(text)?),
        TurnEvent::RestrictedRun(text) => ("restricted_run", non_empty(text)?),
        TurnEvent::RelevantMemories(found) => {
            let shown = &found[..found.len().min(MAX_RECALLED)];
            if shown.is_empty() {
                return None;
            }
            let lines: Vec<String> = shown
                .iter()
                .map(|m| format!("- {}: {}", m.memory.key, m.memory.value))
                .collect();
            // The row keeps the ids it surfaced, so what this session was
            // already shown is folded back from its rows
            // (`memory_context::surfaced_memories`).
            let ids: Vec<i64> = shown.iter().map(|m| m.memory.id).collect();
            return Some(Attachment {
                kind: "relevant_memories",
                text: format!("Memories that may apply:\n{}", lines.join("\n")),
                data: serde_json::Map::from_iter([("ids".to_string(), serde_json::json!(ids))]),
            });
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
            "Plan mode is on: read and research only; nothing else changes, whatever other instructions say. Write the plan with \
             write_plan, then call exit_plan_mode with its path: the owner approves it before anything changes. Don't ask for \
             approval in text; that call is the question.".to_string(),
        ),
        TurnEvent::PlanMode { entered: false } => (
            "plan_mode",
            "Plan mode is off: you can now make changes, run tools and take actions.".to_string(),
        ),
        TurnEvent::GoalSet(condition) => (
            "goal_set",
            format!(
                "Agreed goal: {}. Work continues until a separate check confirms it is met. Briefly acknowledge it, then start (or continue) working toward it now; don't stop to ask.",
                non_empty(condition)?
            ),
        ),
        TurnEvent::GoalCheck { reason, condition } => (
            "goal_check",
            format!("The agreed goal isn't met yet: {}. Keep working toward: {}.", reason.trim(), condition.trim()),
        ),
        TurnEvent::Usage(t) => ("usage", threshold_text(t)),
        TurnEvent::ToolsAvailable(d) => {
            let text = non_empty(&render_listing(d))?;
            let added = d.added.iter().map(|n| (n.clone(), String::new())).collect();
            let mut row = listing_row("tools_available", text, &added, &d.removed);
            row.data.extend(d.replaced_data());
            return Some(row);
        }
        TurnEvent::SkillListing(d) => return d.attachment("skill_listing", &SKILL_WORDS),
        TurnEvent::HelperTypes(d) => return d.attachment("helper_types", &HELPER_WORDS),
        TurnEvent::BackgroundUpdate(text) => ("background_update", non_empty(text)?),
        TurnEvent::CutoffResume => (
            "cutoff_resume",
            "Your last reply hit the output limit. Resume directly, with no apology and no recap: pick up mid-thought if that is where it stopped, and break the remaining work into smaller pieces.".to_string(),
        ),
        TurnEvent::StreamCut => (
            "stream_cut",
            "Your last reply was cut off mid-stream by the connection. Resume directly from where it stopped, with no recap.".to_string(),
        ),
        TurnEvent::EmptyReply => (
            "empty_reply",
            "Your last reply had no visible output. Continue.".to_string(),
        ),
        TurnEvent::WorkflowContract(text) => ("workflow_contract", non_empty(text)?),
        TurnEvent::AppHook { text, .. } => ("app_hook", non_empty(text)?),
        TurnEvent::RestoredFile { path, content } => (
            "restored_file",
            format!("{path} was read earlier in this conversation. Its content now, re-read from disk after the checkpoint:\n\n{content}"),
        ),
        TurnEvent::InvokedSkills(skills) => {
            if skills.is_empty() {
                return None;
            }
            let sections: Vec<String> = skills.iter().map(|(name, content)| format!("### {name}\n{content}")).collect();
            (
                "invoked_skills",
                format!("Skills loaded for this work. Their instructions apply:\n\n{}", sections.join("\n\n")),
            )
        }
        TurnEvent::RunningWork(work) => ("running_work", work.text()),
    };
    Some(Attachment {
        kind,
        text,
        data: serde_json::Map::new(),
    })
}

// ── Session facts ───────────────────────────────────────────────────────

/// A coworker's limit on shared memory, lifted mid-conversation.
const COWORKER_ACCESS_LIFTED: &str =
    "The coworker you're replying to has now been given this employee's shared memory; the earlier limit no longer applies.";

fn fields_json(fields: &[(String, String)]) -> serde_json::Value {
    serde_json::Value::Object(fields.iter().map(|(k, v)| (k.clone(), serde_json::json!(v))).collect())
}

/// The whole environment: the date, then each field.
fn environment_row(f: &SessionFacts) -> Option<Attachment> {
    let date = f.date.format("%A, %B %-d, %Y");
    let date = match &f.timezone {
        Some(tz) => format!("{date} ({tz})"),
        None => date.to_string(),
    };
    let mut lines = vec!["# Environment".to_string(), format!("- Date: {date}")];
    lines.extend(f.environment.iter().map(|(k, v)| format!("- {k}: {v}")));
    let mut data = serde_json::Map::from_iter([("fields".to_string(), fields_json(&f.environment))]);
    data.insert("date".into(), serde_json::json!(f.date.to_string()));
    Some(Attachment { kind: "environment", text: lines.join("\n"), data })
}

fn mode_row(m: &ModeFacts) -> Option<Attachment> {
    Some(Attachment {
        kind: "mode",
        text: format!(
            "Model: {}. Permission mode: {}.",
            m.model, m.permission_mode
        ),
        data: serde_json::Map::from_iter([
            ("model".to_string(), serde_json::json!(m.model)),
            ("mode".to_string(), serde_json::json!(m.permission_mode)),
        ]),
    })
}

/// A fact told whole: the first time plain, later under `lead`. The row
/// keeps a digest of what it told, for the change check.
fn replacement_row(kind: &'static str, text: &str, lead: Option<&str>) -> Option<Attachment> {
    let body = non_empty(text)?;
    let text = match lead {
        Some(lead) => format!("{lead}\n\n{body}"),
        None => body.clone(),
    };
    Some(Attachment {
        kind,
        text,
        data: serde_json::Map::from_iter([("digest".to_string(), serde_json::json!(digest(&body)))]),
    })
}

fn digest(text: &str) -> String {
    format!("{:016x}", super::simple_hash(text.trim().as_bytes()))
}

/// What the conversation was last told about the session, folded from its
/// rows since the boundary; `None` for a fact never told.
#[derive(Debug, Default)]
struct Told {
    identity: Option<String>,
    activity: Option<String>,
    date: Option<String>,
    environment: Option<BTreeMap<String, String>>,
    mode: Option<(String, String)>,
    employee_memory: Option<String>,
    session_context: Option<String>,
    channel_rules: Option<String>,
    coworker_access: Option<String>,
}

fn told(history: &[ChatMessage]) -> Told {
    let mut t = Told::default();
    let text = |f: &serde_json::Map<String, serde_json::Value>, k: &str| f.get(k).and_then(|v| v.as_str()).map(str::to_string);
    for f in history.iter().filter_map(attachment_fields) {
        match f.get("kind").and_then(|k| k.as_str()) {
            Some("environment") => {
                let env = t.environment.get_or_insert_with(BTreeMap::new);
                if let Some(fields) = f.get("fields").and_then(|v| v.as_object()) {
                    for (k, v) in fields {
                        env.insert(k.clone(), v.as_str().unwrap_or_default().to_string());
                    }
                }
                if let Some(date) = text(&f, "date") {
                    t.date = Some(date);
                }
            }
            Some("date_changed") => t.date = text(&f, "date").or(t.date.take()),
            Some("mode") => t.mode = Some((text(&f, "model").unwrap_or_default(), text(&f, "mode").unwrap_or_default())),
            Some("identity") => t.identity = text(&f, "digest"),
            Some("activity") => t.activity = text(&f, "digest"),
            Some("employee_memory") => t.employee_memory = text(&f, "digest"),
            Some("session_context") => t.session_context = text(&f, "digest"),
            Some("channel_rules") => t.channel_rules = text(&f, "digest"),
            Some("coworker_access") => t.coworker_access = text(&f, "digest"),
            _ => {}
        }
    }
    t
}

/// The events that bring the conversation up to date with `now`: the whole
/// snapshot when it was told nothing (a session's first step, or the first
/// after a checkpoint), otherwise one event per fact that changed, and none
/// when nothing did.
pub fn session_fact_events(now: &SessionFacts, history: &[ChatMessage]) -> Vec<TurnEvent> {
    let t = told(history);
    if t.identity.is_none()
        && t.activity.is_none()
        && t.environment.is_none()
        && t.mode.is_none()
        && t.employee_memory.is_none()
        && t.session_context.is_none()
        && t.channel_rules.is_none()
        && t.coworker_access.is_none()
    {
        return vec![TurnEvent::SessionSnapshot(now.clone())];
    }
    let differs = |told: &Option<String>, text: &str| !text.trim().is_empty() && told.as_deref() != Some(digest(text).as_str());
    let mut out = Vec::new();
    if differs(&t.identity, &now.identity) {
        out.push(TurnEvent::IdentityChanged(now.identity.clone()));
    }
    if differs(&t.activity, &now.activity) {
        out.push(TurnEvent::ActivityChanged(now.activity.clone()));
    }
    let env = t.environment.unwrap_or_default();
    let changed: Vec<(String, String)> =
        now.environment.iter().filter(|(k, v)| env.get(k) != Some(v)).cloned().collect();
    if !changed.is_empty() {
        out.push(TurnEvent::EnvironmentChanged(changed));
    }
    if t.date.as_deref() != Some(now.date.to_string().as_str()) {
        out.push(TurnEvent::DateChanged(now.date));
    }
    if t.mode.as_ref() != Some(&(now.mode.model.clone(), now.mode.permission_mode.clone())) {
        out.push(TurnEvent::ModeChanged(now.mode.clone()));
    }
    if differs(&t.employee_memory, &now.employee_memory) {
        out.push(TurnEvent::EmployeeMemoryChanged(now.employee_memory.clone()));
    }
    if differs(&t.session_context, &now.session_context) {
        out.push(TurnEvent::SessionContextChanged(now.session_context.clone()));
    }
    if differs(&t.channel_rules, &now.channel_rules) {
        out.push(TurnEvent::ChannelRulesChanged {
            rules: now.channel_rules.clone(),
            told: t.channel_rules.is_some(),
        });
    }
    if differs(&t.coworker_access, &now.coworker_access) {
        out.push(TurnEvent::CoworkerAccessChanged(now.coworker_access.clone()));
    } else if now.coworker_access.trim().is_empty() && t.coworker_access.as_ref().is_some_and(|d| *d != digest("")) {
        out.push(TurnEvent::CoworkerAccessChanged(String::new()));
    }
    out
}

/// What the conversation is told when the owner's phone position stops
/// being shared with the employee: sharing was turned off, or the phone
/// stopped reporting.
const PHONE_LOCATION_WITHDRAWN: &str = "The owner's phone location is not shared with you now: sharing was turned off \
or the phone stopped reporting. Earlier readings in this conversation are withdrawn; don't use or repeat them.";

/// The phone-position row this step needs: a reading the conversation has
/// not heard, a withdrawal when what it heard is no longer shared, or none.
/// `now` is what is shared with this turn's employee at this moment.
pub fn phone_location_event(
    now: Option<crate::phone_location::SharedPosition>,
    history: &[ChatMessage],
) -> Option<TurnEvent> {
    let told = history
        .iter()
        .filter_map(attachment_fields)
        .filter(|f| f.get("kind").and_then(|k| k.as_str()) == Some("phone_location"))
        .last()
        .map(|f| f.get("taken").and_then(|t| t.as_str()).unwrap_or_default().to_string());
    match now {
        Some(p) if told.as_deref() != Some(p.taken.as_str()) => Some(TurnEvent::PhoneLocation(Some(p))),
        None if told.is_some_and(|t| !t.is_empty()) => Some(TurnEvent::PhoneLocation(None)),
        _ => None,
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

const AGENT_WORDS: ListingWords = ListingWords {
    available: "Other employees on this team. Work that is one of theirs goes to them with send_message; a helper is for work you would do yourself:",
    removed: "These employees are no longer on the team:",
};

const TEAM_WORDS: ListingWords = ListingWords {
    available: "Teams, and the work each owns. A message to a team with send_message goes to its lead, who answers and hands steps to the members; write @Name to ask a member, @everyone to ask the whole team. A team with no lead takes only named asks:",
    removed: "These teams no longer exist:",
};

/// One team's roster line: what it owns, its lead and its members, by name.
pub fn team_line(mission: &str, lead: Option<&str>, members: &[String]) -> String {
    let owns = match mission.trim() {
        "" => "nothing stated yet",
        m => m,
    };
    let lead = match lead {
        Some(name) => format!("lead: {name}"),
        None => "no lead set".to_string(),
    };
    format!("owns {owns}; {lead}; members: {}", members.join(", "))
}

/// Claude Code 2.1.280's agent listing: "Available agent types for the
/// Agent tool:", then "When you launch multiple agents for independent
/// work, send them in a single message with multiple tool uses so they run
/// concurrently." (m0342, `agent_listing_delta`).
const HELPER_WORDS: ListingWords = ListingWords {
    available: "Helper types for delegate, and when each fits. Independent pieces of work are several delegate calls in one response, so they run side by side:",
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

    fn facts() -> SessionFacts {
        SessionFacts {
            identity: "You are Ava, an AI employee working for your owner through Nebo.\n\n# Your job\n\nYou keep the books.".into(),
            activity: String::new(),
            date: chrono::NaiveDate::from_ymd_opt(2026, 9, 24).unwrap(),
            timezone: Some("America/Denver".into()),
            environment: vec![("Platform".into(), "macOS (aarch64)".into()), ("Channel".into(), "web".into())],
            mode: ModeFacts { model: "janus/nebo-1".into(), permission_mode: "Automatic".into() },
            employee_memory: "# User Information\nName: Sam".into(),
            session_context: "# Workspace notes\n\nFiles live in ~/Clients.".into(),
            channel_rules: crate::harness::prompt::sections::channel_rules("web", false, "/data/files"),
            coworker_access: String::new(),
        }
    }

    /// One step's session-fact rows written into `history`; their kinds.
    fn fact_step(history: &mut Vec<ChatMessage>, now: &SessionFacts) -> Vec<&'static str> {
        let rows: Vec<Attachment> = session_fact_events(now, history).iter().flat_map(attachments_for).collect();
        history.extend(rows.iter().map(stored));
        rows.iter().map(|a| a.kind).collect()
    }

    #[test]
    fn session_snapshot_on_first_step_then_deltas_only() {
        let mut history = vec![row("user", "hello", None, None)];
        assert_eq!(
            fact_step(&mut history, &facts()),
            ["identity", "environment", "mode", "employee_memory", "session_context", "channel_rules"]
        );
        assert!(history[2].content.contains("- Date: Thursday, September 24, 2026 (America/Denver)"));
        assert!(fact_step(&mut history, &facts()).is_empty(), "nothing changed, nothing written");
        let mut moved = facts();
        moved.environment[1].1 = "slack".into();
        assert_eq!(fact_step(&mut history, &moved), ["environment"]);
        let delta = history.last().unwrap();
        assert!(delta.content.contains("Environment update:\n- Channel: slack") && !delta.content.contains("Platform"), "{}", delta.content);
        assert!(fact_step(&mut history, &moved).is_empty());
        // After a checkpoint the conversation was told nothing: the snapshot again.
        let mut after = vec![row("user", "summary", None, Some(serde_json::json!({"checkpoint": true})))];
        assert_eq!(fact_step(&mut after, &moved).len(), 6);
    }

    /// How to write for the channel is a session fact: told with the
    /// snapshot, replaced whole when the channel's rules change, and never
    /// system-prompt text.
    #[test]
    fn channel_rules_are_a_session_fact() {
        use crate::harness::prompt::sections::channel_rules;
        let web = channel_rules("web", false, "/data/files");
        for rule in ["Work panel", "write_file", "/data/files", "convert_file", "share_file", "`$$…$$`", "real data"] {
            assert!(web.contains(rule), "the web rules say {rule}: {web}");
        }
        let neboai = channel_rules("neboai", false, "/data/files");
        assert!(neboai.contains("Work panel") && neboai.contains("another computer"), "{neboai}");
        for (channel, rule) in [("dm", "no markdown"), ("cli", "no markdown"), ("voice", "one or two sentences")] {
            let rules = channel_rules(channel, false, "/data/files");
            assert!(rules.contains(rule) && !rules.contains("Work panel"), "{channel}: {rules}");
        }
        let slack = channel_rules("slack", true, "/data/files");
        assert!(slack.contains("plugin__slack") && slack.contains("upload --path"), "{slack}");
        assert!(channel_rules("slack", false, "/data/files").is_empty(), "no channel plugin, no plugin rule");
        assert!(channel_rules("workflow", false, "/data/files").is_empty());
        let prompt = crate::harness::prompt::system_prompt();
        assert!(!prompt.contains("Work panel") && !prompt.contains("upload --path"), "never in the system prompt");

        let mut history = vec![row("user", "hello", None, None)];
        fact_step(&mut history, &facts());
        assert!(history.iter().any(|m| m.content.contains("# Channel rules") && m.content.contains("Work panel")));
        let mut voice = facts();
        voice.channel_rules = channel_rules("voice", false, "/data/files");
        assert_eq!(fact_step(&mut history, &voice), ["channel_rules"]);
        let replaced = &history.last().unwrap().content;
        assert!(replaced.contains("have changed") && replaced.contains("spoken aloud"), "{replaced}");
        assert!(fact_step(&mut history, &voice).is_empty());
    }

    /// A coworker without shared memory hears its limit once; a lifted
    /// limit is said once; an unrestricted asker hears nothing.
    #[test]
    fn a_coworker_without_shared_memory_is_told_the_limit() {
        use crate::harness::prompt::sections::coworker_access;
        let mut history = vec![row("user", "hello", None, None)];
        let mut asking = facts();
        asking.coworker_access = coworker_access(true);
        assert!(fact_step(&mut history, &asking).contains(&"coworker_access"));
        assert!(history.last().unwrap().content.contains("must not be passed on"), "{}", history.last().unwrap().content);
        assert!(fact_step(&mut history, &asking).is_empty(), "told once");
        let granted = facts();
        assert_eq!(fact_step(&mut history, &granted), ["coworker_access"]);
        assert!(history.last().unwrap().content.contains("no longer applies"));
        assert!(fact_step(&mut history, &granted).is_empty(), "lifted once");

        let mut owner = vec![row("user", "hello", None, None)];
        assert!(!fact_step(&mut owner, &facts()).contains(&"coworker_access"), "no limit, no row");
        assert!(coworker_access(false).is_empty());
    }

    /// Who the turn is for is a row: told first, told again after a
    /// checkpoint, and replaced whole when the owner edits the employee.
    #[test]
    fn identity_is_told_once_resent_after_checkpoint_and_replaced_on_edit() {
        let mut history = vec![row("user", "hello", None, None)];
        assert_eq!(fact_step(&mut history, &facts())[0], "identity");
        assert_eq!(history[1].content, crate::harness::reminders::wrap(&facts().identity), "told plain, first");
        assert!(fact_step(&mut history, &facts()).is_empty(), "told once");
        let mut edited = facts();
        edited.identity = edited.identity.replace("keep the books", "run payroll");
        assert_eq!(fact_step(&mut history, &edited), ["identity"]);
        let replaced = &history.last().unwrap().content;
        assert!(replaced.contains("Who you are has changed; this replaces the earlier version:") && replaced.contains("run payroll"), "{replaced}");
        assert!(fact_step(&mut history, &edited).is_empty());
        let mut after = vec![row("user", "summary", None, Some(serde_json::json!({"checkpoint": true})))];
        assert_eq!(fact_step(&mut after, &edited)[0], "identity", "re-sent after the checkpoint");
        assert!(after[1].content.contains("run payroll") && !after[1].content.contains("has changed"), "{}", after[1].content);
    }

    #[test]
    fn activity_instructions_ride_after_the_identity() {
        let mut history = Vec::new();
        let mut activity = facts();
        activity.activity = "## Task\nReconcile the ledger.".into();
        assert_eq!(fact_step(&mut history, &activity)[..2], ["identity", "activity"]);
        assert!(history[1].content.contains("Reconcile the ledger."));
        assert!(fact_step(&mut history, &activity).is_empty());
    }

    #[test]
    fn mode_switch_writes_one_mode_row() {
        let mut history = Vec::new();
        fact_step(&mut history, &facts());
        let mut plan = facts();
        plan.mode.permission_mode = "Plan".into();
        assert_eq!(fact_step(&mut history, &plan), ["mode"]);
        let row = &history.last().unwrap().content;
        assert!(row.contains(&format!("Model: {}. Permission mode: Plan.", plan.mode.model)), "{row}");
        assert!(!row.contains("guess"), "facts only, as Claude Code states the model: {row}");
        assert!(fact_step(&mut history, &plan).is_empty());
    }

    #[test]
    fn memory_write_replaces_employee_memory_next_step() {
        let mut history = Vec::new();
        fact_step(&mut history, &facts());
        let mut wrote = facts();
        wrote.employee_memory.push_str("\nPrefers mornings");
        assert_eq!(fact_step(&mut history, &wrote), ["employee_memory"]);
        let row = &history.last().unwrap().content;
        assert!(row.contains("Your memory has changed; this replaces the earlier version:") && row.contains("Prefers mornings"), "{row}");
        assert!(fact_step(&mut history, &wrote).is_empty());
    }

    #[test]
    fn date_roll_writes_date_changed() {
        let mut history = Vec::new();
        fact_step(&mut history, &facts());
        let mut tomorrow = facts();
        tomorrow.date = tomorrow.date.succ_opt().unwrap();
        assert_eq!(fact_step(&mut history, &tomorrow), ["date_changed"]);
        assert!(history.last().unwrap().content.contains("Today is Friday, September 25, 2026."));
        assert!(fact_step(&mut history, &tomorrow).is_empty(), "told once");
    }

    fn every_event() -> Vec<TurnEvent> {
        let lined = LinedDelta::between(&Listing::new(), &Listing::from([("x".to_string(), "y".to_string())])).unwrap();
        vec![
            TurnEvent::SessionSnapshot(facts()),
            TurnEvent::ActivityChanged("## Task\nReconcile the ledger.".into()),
            TurnEvent::AgentsListing(lined.clone()),
            TurnEvent::TeamsListing(lined.clone()),
            TurnEvent::StreamCut,
            TurnEvent::EmptyReply,
            TurnEvent::DateChanged(chrono::NaiveDate::from_ymd_opt(2026, 9, 24).unwrap()),
            TurnEvent::RunBriefing("team: Ann".into()),
            TurnEvent::RestrictedRun("outside origin".into()),
            TurnEvent::RelevantMemories(vec![scored("k", "v")]),
            TurnEvent::FilesChanged(vec!["a.rs changed".into()]),
            TurnEvent::Diagnostics(vec!["a.rs:1 error".into()]),
            TurnEvent::TasksIdle(Vec::new()),
            TurnEvent::PlanMode { entered: true },
            TurnEvent::GoalSet("all tests pass".into()),
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
            TurnEvent::RestoredFile {
                path: "/tmp/a.txt".into(),
                content: "a".into(),
            },
            TurnEvent::InvokedSkills(vec![("letters".into(), "write plainly".into())]),
            TurnEvent::RunningWork(crate::harness::compact::restore::RunningWork {
                id: "task-1".into(),
                description: "research".into(),
                kind: crate::harness::compact::restore::WorkKind::Helper,
            }),
            TurnEvent::TurnTime("It is 2:05 PM (America/Denver, UTC-06:00) on Thursday, September 24, 2026.".into()),
            TurnEvent::PhoneLocation(Some(crate::phone_location::SharedPosition {
                text: "The owner's phone is at 40.000000, -111.000000.".into(),
                taken: "phone@1000".into(),
            })),
            TurnEvent::PhoneLocation(None),
            TurnEvent::ChannelRulesChanged {
                rules: "# Channel rules\nNo markdown.".into(),
                told: true,
            },
            TurnEvent::CoworkerAccessChanged(String::new()),
        ]
    }

    #[test]
    fn every_event_has_one_named_row() {
        let mut kinds: Vec<&str> = every_event()
            .iter()
            .flat_map(|e| {
                let rows = attachments_for(e);
                assert!(!rows.is_empty(), "{e:?} speaks");
                rows
            })
            .map(|a| a.kind)
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
            "The date has changed. Today is Thursday, September 24, 2026. No need to announce it."
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
            let a = attachment_for(&TurnEvent::ToolsAvailable(ToolsDelta::between(&told, now, Default::default())?))?;
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
