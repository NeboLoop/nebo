//! What a checkpoint re-attaches after its boundary: the files read most
//! recently (re-read fresh from disk), the skills loaded with their
//! instructions, the agreed goal, the work still running and plan mode. Each
//! is a turn event, so it reaches the conversation the one way every
//! attachment does (`events::attachment_for`, `Reminders::write`). The
//! conversation before the boundary is where the files and skills are
//! found; the caller supplies what only the turn knows.

use std::collections::HashSet;

use db::models::ChatMessage;

use crate::harness::events::TurnEvent;
use crate::harness::goal::{AgreedGoal, GoalStatus};

/// Most files re-attached.
pub const MAX_FILES: usize = 5;
/// Most tokens of one re-attached file.
pub const FILE_TOKENS: usize = 5_000;
/// Most tokens of every re-attached file together.
pub const FILES_TOKENS: usize = 50_000;
/// Most tokens of one re-attached skill.
pub const SKILL_TOKENS: usize = 5_000;
/// Most tokens of every re-attached skill together.
pub const SKILLS_TOKENS: usize = 25_000;

const CUT_NOTE: &str = "\n…(cut to fit after the checkpoint; read it again for the rest)";

/// Work started before the checkpoint that has not finished: a helper, a
/// background shell, a workflow run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningWork {
    pub id: String,
    pub description: String,
    /// Where it stands, one line.
    pub status: String,
}

/// What the turn knows that the conversation does not carry.
#[derive(Debug, Clone, Default)]
pub struct RestoreState<'a> {
    pub goal: Option<&'a AgreedGoal>,
    pub running: &'a [RunningWork],
    pub plan_mode: bool,
}

/// The restore list, in order: files newest first, skills, the goal, running
/// work, plan mode. `before` is the conversation the checkpoint summarized,
/// as stored.
pub fn restore(before: &[ChatMessage], state: &RestoreState<'_>) -> Vec<TurnEvent> {
    let mut events = Vec::new();

    let (mut files, mut files_tokens) = (0, 0);
    for path in recent_file_reads(before) {
        if files == MAX_FILES {
            break;
        }
        let Ok(content) = std::fs::read_to_string(&path) else { continue };
        let content = clip(&content, FILE_TOKENS);
        let tokens = tokens(&content);
        if files_tokens + tokens > FILES_TOKENS {
            continue;
        }
        files += 1;
        files_tokens += tokens;
        events.push(TurnEvent::RestoredFile { path, content });
    }

    let mut skills_tokens = 0;
    let mut skills = Vec::new();
    for (name, content) in loaded_skills(before) {
        let content = clip(&content, SKILL_TOKENS);
        let tokens = tokens(&content);
        if skills_tokens + tokens > SKILLS_TOKENS {
            continue;
        }
        skills_tokens += tokens;
        skills.push((name, content));
    }
    if !skills.is_empty() {
        events.push(TurnEvent::InvokedSkills(skills));
    }

    if let Some(goal) = state.goal.filter(|g| g.status == GoalStatus::Active) {
        events.push(TurnEvent::GoalSet(goal.condition.clone()));
    }

    for work in state.running {
        events.push(TurnEvent::RunningWork {
            id: work.id.clone(),
            description: work.description.clone(),
            status: work.status.clone(),
        });
    }

    if state.plan_mode {
        events.push(TurnEvent::PlanMode { entered: true });
    }

    events
}

fn tokens(text: &str) -> usize {
    text.len() / crate::CHARS_PER_TOKEN
}

/// `text` cut to `max_tokens` at a char boundary, with a note when cut.
fn clip(text: &str, max_tokens: usize) -> String {
    let max = max_tokens * crate::CHARS_PER_TOKEN;
    if text.len() <= max {
        return text.to_string();
    }
    let mut cut = max.saturating_sub(CUT_NOTE.len());
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{CUT_NOTE}", &text[..cut])
}

/// Every tool call in `messages`, newest first: (call id, tool name, input).
fn calls_newest_first(messages: &[ChatMessage]) -> Vec<(String, String, serde_json::Value)> {
    let mut calls = Vec::new();
    for msg in messages.iter().rev().filter(|m| m.role == "assistant") {
        let Some(parsed) = msg
            .tool_calls
            .as_deref()
            .and_then(|tc| serde_json::from_str::<Vec<serde_json::Value>>(tc).ok())
        else {
            continue;
        };
        for call in parsed.iter().rev() {
            let field = |k: &str| call.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
            calls.push((field("id"), field("name"), call.get("input").cloned().unwrap_or_default()));
        }
    }
    calls
}

/// The absolute paths of files read with a whole-file read, newest first,
/// each once.
fn recent_file_reads(messages: &[ChatMessage]) -> Vec<String> {
    let mut seen = HashSet::new();
    calls_newest_first(messages)
        .into_iter()
        .filter(|(_, name, _)| name == "read_file")
        .filter_map(|(_, _, input)| input.get("path").and_then(|v| v.as_str()).map(str::to_string))
        .filter(|p| std::path::Path::new(p).is_absolute() && seen.insert(p.clone()))
        .collect()
}

/// Skills loaded, newest first, each once: (name, the content the load
/// returned). A load that failed loaded nothing.
fn loaded_skills(messages: &[ChatMessage]) -> Vec<(String, String)> {
    let results = results_by_call(messages);
    let mut decided = HashSet::new();
    let mut skills = Vec::new();
    for (id, name, input) in calls_newest_first(messages) {
        let skill = input.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if name != tools::skill_tool::USE_SKILL || skill.is_empty() || decided.contains(skill) {
            continue;
        }
        if let Some((content, false)) = results.get(id.as_str()) {
            decided.insert(skill.to_string());
            skills.push((skill.to_string(), content.clone()));
        }
    }
    skills
}

/// Each tool result in `messages` by the call it answers: (content, is_error).
fn results_by_call(messages: &[ChatMessage]) -> std::collections::HashMap<String, (String, bool)> {
    let mut map = std::collections::HashMap::new();
    for msg in messages {
        let Some(rows) = msg
            .tool_results
            .as_deref()
            .and_then(|tr| serde_json::from_str::<Vec<serde_json::Value>>(tr).ok())
        else {
            continue;
        };
        for row in rows {
            let Some(id) = row.get("tool_call_id").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) else {
                continue;
            };
            let content = row.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let is_error = row.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
            map.insert(id.to_string(), (content, is_error));
        }
    }
    map
}
