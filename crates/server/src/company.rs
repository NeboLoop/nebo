//! What everyone in the company is doing right now ([`tools::company`]),
//! read where each employee's work runs, and which of a linked employee's
//! conversations a coworker message goes into.
//!
//! A native employee's work is in this Nebo: its live runs (the run
//! registry), its duties (live workflow runs), and the questions it waits on
//! the owner for (parked questions and permission asks). A linked employee's
//! work is on the computer it runs on: that host says which of its
//! conversations are working and why (`host/status`), and a conversation's
//! record says what the work is (`session/load`). Both are read through the
//! linked provider, the same way its turns reach it.

use std::collections::HashMap;
use std::time::Duration;

use ai::providers::linked::{AgentStatus, Life, LinkedProvider, SessionStatus, Working};
use tools::company::{CompanyQuery, Doing, EmployeeNow, Work};
use tools::coworker::Conversation;

use crate::state::AppState;

/// How long a linked employee's computer is waited on for what it is doing.
const LINKED_WAIT: Duration = Duration::from_secs(5);
/// The latest rows of a native employee's conversation its detail carries,
/// and the most of one row.
const DETAIL_ROWS: i64 = 4;
const DETAIL_ROW_CHARS: usize = 300;

/// What every employee is doing right now, in roster order.
pub(crate) async fn now(state: AppState, q: CompanyQuery) -> Vec<EmployeeNow> {
    let rows = state.store.list_agents(500, 0).unwrap_or_default();
    let runs: Vec<_> = state
        .run_registry
        .list_all()
        .await
        .into_iter()
        .filter(|r| r.parent_run_id.is_none() && r.session_key != q.caller_session)
        .collect();
    let parked: HashMap<String, String> = state
        .run_registry
        .pending_asks()
        .await
        .into_iter()
        .map(|(key, ask)| (key, ask.prompt))
        .collect();
    let asks = state.permission_asks.open(None).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "company: the open asks could not be read");
        Vec::new()
    });
    let duties = state
        .store
        .engine_live_runs_of_kind("workflow")
        .unwrap_or_default();
    let caller = caller_of(&q.caller_session);
    let mut linked = linked_now(&state, &rows, q.detail_for.as_deref(), &caller, &parked).await;
    let at = chrono::Utc::now().timestamp();

    let mut out = Vec::new();
    for row in &rows {
        if let Some(employee) = linked.remove(&row.id) {
            out.push(employee);
            continue;
        }
        let detail = q.detail_for.as_deref() == Some(row.id.as_str());
        let readable = conversations_readable(&row.frontmatter, &caller);
        let mut work: Vec<(String, Work)> = Vec::new();
        for r in runs.iter().filter(|r| entity(&r.entity_id) == row.id) {
            let doing = if !r.activity.is_empty() {
                r.activity.clone()
            } else if !r.current_tool.is_empty() {
                format!("running {}", r.current_tool)
            } else {
                "thinking".to_string()
            };
            work.push((
                r.session_key.clone(),
                Work {
                    place: place_of(&state, &r.session_key, &r.channel, &r.entity_name),
                    doing,
                    since: Some(format!("for {}", span(r.elapsed_secs))),
                    asks: parked.get(&r.session_key).cloned(),
                    detail: match (detail, readable) {
                        (true, true) => recent(&state, &r.session_key),
                        (true, false) => vec![KEPT_APART.to_string()],
                        (false, _) => Vec::new(),
                    },
                    ..Work::default()
                },
            ));
        }
        for d in duties.iter().filter(|d| d.agent_id == row.id) {
            if work
                .iter()
                .any(|(key, _)| !d.session_key.is_empty() && *key == d.session_key)
            {
                continue;
            }
            let run = state.store.get_workflow_run(&d.id).ok().flatten();
            let binding = run
                .as_ref()
                .and_then(|w| w.trigger_detail.as_deref())
                .map(|t| t.split(':').next().unwrap_or(t).to_string())
                .filter(|b| !b.is_empty())
                .unwrap_or_else(|| "a duty".to_string());
            let doing = run
                .as_ref()
                .and_then(|w| w.current_activity.clone())
                .filter(|a| !a.is_empty())
                .unwrap_or_else(|| {
                    match d.state.as_str() {
                        "waiting" => "waiting",
                        "queued" => "about to start",
                        "interrupted" => "picking up where it stopped",
                        _ => "running",
                    }
                    .to_string()
                });
            work.push((
                d.session_key.clone(),
                Work {
                    place: format!("its duty {binding}"),
                    doing,
                    since: d
                        .started_at
                        .map(|s| format!("for {}", span((at - s).max(0) as u64))),
                    ..Work::default()
                },
            ));
        }
        for ask in asks.iter().filter(|a| a.agent_id == row.id) {
            match work.iter_mut().find(|(key, _)| *key == ask.session_key) {
                Some((_, w)) => w.asks = Some(ask.sentence.clone()),
                None => work.push((
                    ask.session_key.clone(),
                    Work {
                        place: place_of(&state, &ask.session_key, "", ""),
                        doing: "stopped until you answer".to_string(),
                        since: Some(format!(
                            "asked {} ago",
                            span((at - ask.created_at).max(0) as u64)
                        )),
                        asks: Some(ask.sentence.clone()),
                        ..Work::default()
                    },
                )),
            }
        }
        let work: Vec<Work> = work.into_iter().map(|(_, w)| w).collect();
        let doing = if work.iter().any(|w| w.asks.is_some()) {
            Doing::WaitingOnOwner
        } else if work.is_empty() {
            Doing::Idle
        } else {
            Doing::Working
        };
        let note = (row.is_enabled == 0 && work.is_empty()).then(|| "turned off".to_string());
        out.push(EmployeeNow {
            agent_id: row.id.clone(),
            name: row.name.clone(),
            doing,
            computer: None,
            note,
            work,
        });
    }
    out
}

/// What a detail says in place of the words of an employee's conversations
/// that are not shared with the one asking.
const KEPT_APART: &str = "Its conversations are kept apart and not shared with you, so their words are not \
     read here. send_message asks it.";

/// The employee asking, from its session: `main` when it is none.
fn caller_of(session_key: &str) -> String {
    let id = types::keyparser::extract_agent_id(session_key);
    if id.is_empty() {
        "main".to_string()
    } else {
        id
    }
}

/// Whether the conversations of the employee `frontmatter` configures may
/// be read into the caller's: an
/// employee whose conversations are kept apart (its memory mode) shares
/// their words only with a colleague its memory settings grant
/// (`memory.share_with`), the rule its recall answers colleagues by. A
/// configuration that can't be read keeps them apart, as its memory does.
fn conversations_readable(frontmatter: &str, caller: &str) -> bool {
    if frontmatter.is_empty() {
        return true;
    }
    match napp::agent::parse_agent_config(frontmatter) {
        Ok(config) => {
            !config.memory.mode.separates_conversations()
                || config
                    .memory
                    .share_with
                    .iter()
                    .any(|g| g == caller || g == "*")
        }
        Err(_) => false,
    }
}

/// The employee a run belongs to: runs of the main entity are the primary
/// employee's.
fn entity(entity_id: &str) -> &str {
    if entity_id == "main" {
        tools::team_tool::PRIMARY_AGENT_ID
    } else {
        entity_id
    }
}

/// Where a run is, as the owner knows it: its conversation's title, or its
/// schedule.
fn place_of(state: &AppState, session_key: &str, channel: &str, entity_name: &str) -> String {
    if channel == "cron" {
        return format!(
            "its scheduled job {}",
            entity_name.trim_start_matches("Cron: ")
        );
    }
    match chat_of(state, session_key)
        .map(|c| c.title)
        .filter(|t| !t.trim().is_empty())
    {
        Some(title) => format!("the conversation \"{}\"", title.trim()),
        None => "a conversation".to_string(),
    }
}

fn chat_of(state: &AppState, session_key: &str) -> Option<db::models::Chat> {
    let sessions = state.harness.sessions();
    let id = sessions.resolve_session_id_by_key(session_key).ok()?;
    state
        .store
        .get_chat(&sessions.active_chat_id(&id))
        .ok()
        .flatten()
}

/// A native employee's conversation, its latest rows, bounded.
fn recent(state: &AppState, session_key: &str) -> Vec<String> {
    let Some(chat) = chat_of(state, session_key) else {
        return Vec::new();
    };
    state
        .store
        .get_recent_chat_messages(&chat.id, DETAIL_ROWS)
        .unwrap_or_default()
        .into_iter()
        .map(|m| {
            let who = if m.role == "user" { "Message" } else { "Said" };
            format!("{who}: \"{}\"", clip(m.content.trim(), DETAIL_ROW_CHARS))
        })
        .collect()
}

fn clip(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{} …", &text[..cut]),
        None => text.to_string(),
    }
}

/// A span of seconds in the fewest words: "12s", "3m", "2h 5m", "3d".
fn span(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => match (s % 3600) / 60 {
            0 => format!("{}h", s / 3600),
            m => format!("{}h {m}m", s / 3600),
        },
        s => format!("{}d", s / 86_400),
    }
}

/// Seconds since an RFC 3339 moment.
fn ago(moment: &str) -> Option<u64> {
    let then = chrono::DateTime::parse_from_rfc3339(moment).ok()?;
    Some((chrono::Utc::now().timestamp() - then.timestamp()).max(0) as u64)
}

/// Why a linked agent or conversation works, in words.
fn why_words(why: &[Working]) -> String {
    let words: Vec<&str> = why
        .iter()
        .map(|w| match w {
            Working::Prompt => "working on a request",
            Working::Tool => "a tool running",
            Working::Permission => "waiting for your answer to a permission request",
            Working::Plan => "working through its plan",
            Working::Request => "opening a conversation",
            Working::Processes => "a process it started still running",
        })
        .collect();
    if words.is_empty() {
        "working".to_string()
    } else {
        words.join(", ")
    }
}

/// The linked provider, as the turns use it.
async fn linked_provider(state: &AppState) -> Option<LinkedProvider> {
    state
        .harness
        .providers()
        .read()
        .await
        .iter()
        .find_map(|p| p.linked().cloned())
}

/// A linked employee's brain: its linked bot and agent.
pub(crate) fn linked_target(state: &AppState, agent_id: &str) -> Option<(String, String)> {
    let model = state
        .store
        .get_entity_config("agent", agent_id)
        .ok()
        .flatten()?
        .model_preference?;
    let (bot, agent) = LinkedProvider::target(&model)?;
    Some((bot.to_string(), agent.to_string()))
}

/// Every linked employee's state, from the computers they run on, by
/// employee id.
async fn linked_now(
    state: &AppState,
    rows: &[db::models::Agent],
    detail_for: Option<&str>,
    caller: &str,
    parked: &HashMap<String, String>,
) -> HashMap<String, EmployeeNow> {
    let mut out = HashMap::new();
    let employees: Vec<(&db::models::Agent, String, String)> = rows
        .iter()
        .filter(|r| r.kind.as_deref() == Some("linked"))
        .filter_map(|r| linked_target(state, &r.id).map(|(bot, agent)| (r, bot, agent)))
        .collect();
    if employees.is_empty() {
        return out;
    }
    let provider = linked_provider(state).await;
    // Another computer's name and whether it is online, from the hub, asked
    // once and only when one runs elsewhere.
    let elsewhere = employees
        .iter()
        .any(|(_, bot, _)| !provider.as_ref().is_some_and(|p| p.hosted_here(bot)));
    let hub = if elsewhere {
        managed_bots(state).await
    } else {
        None
    };

    let mut by_bot: HashMap<&str, Vec<&(&db::models::Agent, String, String)>> = HashMap::new();
    for e in &employees {
        by_bot.entry(e.1.as_str()).or_default().push(e);
    }
    let reads = by_bot.into_iter().map(|(bot, members)| {
        let provider = provider.clone();
        let hub = hub.as_ref();
        async move {
            let here = provider.as_ref().is_some_and(|p| p.hosted_here(bot));
            let listed = hub.and_then(|bots| bots.iter().find(|b| b.id == bot));
            let computer = if here {
                "this computer".to_string()
            } else {
                listed.map(|b| b.name.clone()).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "its computer".to_string())
            };
            let agents: Vec<String> = members.iter().map(|m| m.2.clone()).collect();
            // Not asked: its computer is offline (the hub says so), or it
            // can't be reached from here at all.
            let status = match (&provider, listed) {
                (_, Some(b)) if !here && !b.online => Err((Doing::NotRunning, "its computer is offline")),
                (None, _) => Err((Doing::Unknown, "its computer can't be reached from here")),
                (Some(p), _) => match tokio::time::timeout(LINKED_WAIT, p.status(bot, &agents)).await {
                    Ok(Ok(listed)) => Ok(listed),
                    Ok(Err(e)) => {
                        tracing::info!(bot, error = %e, "company: a linked computer did not say what its agents are doing");
                        Err((Doing::Unknown, "its computer did not answer, so what it is doing is not known"))
                    }
                    Err(_) => Err((Doing::Unknown, "its computer did not answer in time, so what it is doing is not known")),
                },
            };
            (bot, members, computer, status)
        }
    });
    for (bot, members, computer, status) in futures::future::join_all(reads).await {
        for (i, (row, _, agent)) in members.into_iter().enumerate() {
            let mut employee = EmployeeNow {
                agent_id: row.id.clone(),
                name: row.name.clone(),
                doing: Doing::Unknown,
                computer: Some(computer.clone()),
                note: None,
                work: Vec::new(),
            };
            match &status {
                Err((doing, why)) => {
                    employee.doing = *doing;
                    employee.note = Some(why.to_string());
                }
                Ok(listed) => match listed.get(i).cloned().flatten() {
                    None => {
                        employee.doing = Doing::NotRunning;
                        employee.note = Some("it is no longer on its computer".to_string());
                    }
                    Some(s) => {
                        let detail = detail_for == Some(row.id.as_str());
                        let readable = conversations_readable(&row.frontmatter, caller);
                        let p = provider.as_ref().filter(|_| detail && readable);
                        linked_employee(state, &mut employee, &s, bot, agent, p, parked).await;
                        if detail && !readable {
                            for w in &mut employee.work {
                                w.detail.push(KEPT_APART.to_string());
                            }
                        }
                    }
                },
            }
            out.insert(row.id.clone(), employee);
        }
    }
    out
}

/// One linked employee's state from its host's status; `detail` reads each
/// working conversation's record.
async fn linked_employee(
    state: &AppState,
    employee: &mut EmployeeNow,
    s: &AgentStatus,
    bot: &str,
    agent: &str,
    detail: Option<&LinkedProvider>,
    parked: &HashMap<String, String>,
) {
    if s.state == Life::Paused {
        employee.doing = Doing::NotRunning;
        employee.note = Some(
            "paused; its conversations are kept, and a message to it starts it again".to_string(),
        );
        return;
    }
    if !s.busy {
        employee.doing = Doing::Idle;
        employee.note = s
            .idle_since
            .as_deref()
            .and_then(ago)
            .map(|secs| format!("for {}", span(secs)));
        return;
    }
    let busy: Vec<&SessionStatus> = s.sessions.iter().filter(|x| x.busy).collect();
    for session in &busy {
        let chat = state
            .store
            .chat_for_linked_session(agent, &session.session_id)
            .ok()
            .flatten();
        let mut work = Work {
            place: match chat
                .as_ref()
                .map(|c| c.title.trim())
                .filter(|t| !t.is_empty())
            {
                Some(title) => format!("the conversation \"{title}\""),
                None => "a conversation started outside Nebo".to_string(),
            },
            conversation: Some(session.session_id.clone()),
            doing: why_words(&session.why),
            since: session
                .last_update
                .as_deref()
                .and_then(ago)
                .map(|secs| format!("last update {} ago", span(secs))),
            asks: chat
                .as_ref()
                .and_then(|c| c.session_name.as_deref())
                .and_then(|key| parked.get(key))
                .cloned(),
            detail: Vec::new(),
        };
        if let Some(provider) = detail.filter(|_| session.state == Life::Running) {
            match tokio::time::timeout(
                LINKED_WAIT,
                provider.session_work(bot, agent, &session.session_id),
            )
            .await
            {
                Ok(Ok(read)) => {
                    if chat.is_none()
                        && let Some(title) = read.title.as_deref()
                    {
                        work.place = format!("the conversation \"{title}\"");
                    }
                    if let Some(started) = read.running_since.as_deref().and_then(ago) {
                        work.since = Some(format!(
                            "for {}{}",
                            span(started),
                            work.since
                                .as_deref()
                                .map(|s| format!(", {s}"))
                                .unwrap_or_default()
                        ));
                    }
                    if let Some(prompt) = read.prompt {
                        work.detail.push(format!("Working on: \"{prompt}\""));
                    }
                    if !read.calls.is_empty() {
                        let calls: Vec<String> = read
                            .calls
                            .iter()
                            .map(|(l, s)| format!("{l} ({s})"))
                            .collect();
                        let earlier = match read.earlier_calls {
                            0 => String::new(),
                            n => format!(" — after {n} earlier call(s)"),
                        };
                        work.detail
                            .push(format!("Calls: {}{earlier}", calls.join(", ")));
                    }
                    if let Some(said) = read.said {
                        work.detail.push(format!("Latest words: \"{said}\""));
                    }
                }
                Ok(Err(e)) => work
                    .detail
                    .push(format!("Its conversation could not be read: {e}.")),
                Err(_) => work
                    .detail
                    .push("Its conversation could not be read in time.".to_string()),
            }
        }
        employee.work.push(work);
    }
    if busy.is_empty() {
        // Working, but in no conversation it holds: opening one, or a
        // process it started.
        employee.work.push(Work {
            place: "its computer".to_string(),
            doing: why_words(&s.why),
            ..Work::default()
        });
    }
    let asks_owner =
        s.why.contains(&Working::Permission) || employee.work.iter().any(|w| w.asks.is_some());
    employee.doing = if asks_owner {
        Doing::WaitingOnOwner
    } else {
        Doing::Working
    };
}

/// The owner's bots as the hub lists them, when it can be asked.
async fn managed_bots(state: &AppState) -> Option<Vec<comm::api_types::ManagedBot>> {
    let api = crate::codes::build_api_client(state).ok()?;
    match tokio::time::timeout(LINKED_WAIT, api.list_managed_bots()).await {
        Ok(Ok(bots)) => Some(bots),
        Ok(Err(e)) => {
            tracing::info!(error = %e, "company: the hub's bot list did not answer");
            None
        }
        Err(_) => None,
    }
}

/// Point the sender's thread with the linked employee `to_id` at the
/// conversation a coworker message names, before the message runs there:
/// one it holds (never one whose turn is running: a conversation takes one
/// message at a time), or a new one. `Err` is what the sender is told;
/// nothing is sent then.
pub(crate) async fn open_conversation(
    state: &AppState,
    to_id: &str,
    to_name: &str,
    thread_session: &str,
    conversation: &Conversation,
) -> Result<(), String> {
    let Some((bot, agent)) = linked_target(state, to_id) else {
        return Err(format!(
            "{to_name} is not a linked employee: your messages to it go to its one thread with you. \
             Nothing was sent. Send it again without `conversation`."
        ));
    };
    let chat = state.harness.sessions().active_chat_id(thread_session);
    let session = match conversation {
        Conversation::New => String::new(),
        Conversation::Existing(id) => {
            let provider = linked_provider(state)
                .await
                .ok_or_else(|| format!("Could not connect to {to_name}. Try again."))?;
            let status = match tokio::time::timeout(
                LINKED_WAIT,
                provider.status(&bot, std::slice::from_ref(&agent)),
            )
            .await
            {
                Ok(Ok(mut listed)) => listed.pop().flatten(),
                _ => return Err(format!("Could not connect to {to_name}. Try again.")),
            };
            let held = status
                .as_ref()
                .and_then(|s| s.sessions.iter().find(|x| x.session_id == *id));
            match held {
                None => {
                    return Err(format!(
                        "{to_name} has no conversation {id} on its computer. list_employees shows the ones it is \
                         working in. Nothing was sent."
                    ));
                }
                Some(x) if x.busy => {
                    return Err(format!(
                        "{to_name} is in the middle of a turn in that conversation ({}). Nothing was sent: a \
                         conversation takes one message at a time. Send it when that turn ends, or start another \
                         with conversation: \"new\".",
                        why_words(&x.why)
                    ));
                }
                Some(_) => id.clone(),
            }
        }
    };
    let agent_field = if session.is_empty() {
        ""
    } else {
        agent.as_str()
    };
    state
        .store
        .set_chat_linked_session(&chat, agent_field, &session)
        .map_err(|e| format!("failed to open that conversation with {to_name}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_are_said_in_the_fewest_words() {
        assert_eq!(span(12), "12s");
        assert_eq!(span(180), "3m");
        assert_eq!(span(7200), "2h");
        assert_eq!(span(7500), "2h 5m");
        assert_eq!(span(3 * 86_400 + 5), "3d");
    }

    /// An employee's conversations are read by a colleague only as its
    /// memory settings allow: one running conversation is readable; kept
    /// apart, only by a colleague `share_with` grants.
    #[test]
    fn kept_apart_conversations_are_read_only_by_a_granted_colleague() {
        assert!(conversations_readable("", "assistant"));
        assert!(
            conversations_readable("{}", "assistant"),
            "a linked employee's config"
        );
        assert!(conversations_readable(
            r#"{"memory":{"mode":"single"}}"#,
            "assistant"
        ));
        for sealed in [
            r#"{"memory":{"mode":"confidential"}}"#,
            r#"{"memory":{"mode":"separate"}}"#,
            r#"{"memory":{"context_isolated":true}}"#,
        ] {
            assert!(!conversations_readable(sealed, "assistant"), "{sealed}");
        }
        assert!(conversations_readable(
            r#"{"memory":{"mode":"confidential","share_with":["assistant"]}}"#,
            "assistant"
        ));
        assert!(!conversations_readable(
            r#"{"memory":{"mode":"confidential","share_with":["books"]}}"#,
            "assistant"
        ));
        assert!(conversations_readable(
            r#"{"memory":{"mode":"separate","share_with":["*"]}}"#,
            "assistant"
        ));
        assert!(
            !conversations_readable("not json", "assistant"),
            "unreadable settings keep them apart"
        );
        assert_eq!(caller_of("agent:assistant:thread:c1"), "assistant");
        assert_eq!(caller_of("proof:dm:x"), "main");
    }

    #[test]
    fn why_a_linked_agent_works_is_said_in_words() {
        assert_eq!(
            why_words(&[Working::Prompt, Working::Tool]),
            "working on a request, a tool running"
        );
        assert_eq!(
            why_words(&[Working::Permission]),
            "waiting for your answer to a permission request"
        );
        assert_eq!(why_words(&[]), "working");
    }
}
