//! The `expert` activity: a workflow step done by another expert agent.
//!
//! A coworker on this bot does the work as an ASSIGNMENT (`cases::open_assignment`):
//! its own case, run durably by the engine, closed with a status and a
//! summary. The step waits durably for that close: the request is one row on
//! the effects ledger (sent once, whatever happens to the process), the reply
//! is one engine event aimed at the run (`expert_reply` on `expert:<run>`),
//! and while any expert is still out the run is parked on one engine wait
//! that the next reply or the earliest deadline wakes. The woken run re-enters
//! under its own id; finished nodes replay and only unfinished work runs.
//!
//! An expert on another bot (a `loop:` address) is never sent anything: data
//! leaving the owner's bot needs his approval per workflow and expert, and
//! there is no approval of that kind yet, so the step is blocked and says so.

use db::{NewEvent, Store};
use serde_json::Value;

use crate::parser::{Activity, param_str};

/// Engine event kind a reply arrives as.
pub const REPLY_KIND: &str = "expert_reply";
/// The effects-ledger class of an expert request.
pub const EFFECT_CLASS: &str = "expert";
/// The prefix that names an agent on another bot.
pub const LOOP_PREFIX: &str = "loop:";
/// Longest summary a node output carries.
const SUMMARY_MAX: usize = 280;

/// What every expert reply for one run is aimed at; the run's wait listens
/// on it.
pub fn run_target(run_id: &str) -> String {
    format!("expert:{run_id}")
}

/// One request's identity: the run, the node, the loop iteration and the
/// attempt (`on_error.retry` sends again under the next attempt).
pub fn request_key(run_id: &str, activity_id: &str, iteration: &str, attempt: u32) -> String {
    format!("{run_id}:{activity_id}:{iteration}:{attempt}")
}

fn effect_key(key: &str) -> String {
    format!("expert:{key}")
}

fn reply_idem(key: &str) -> String {
    format!("expert-reply:{key}")
}

/// `params.timeout`: `30s`, `45m`, `2h`, `3d`. Required — an expert that
/// never answers must not hold the run forever.
pub fn parse_timeout(s: &str) -> Option<std::time::Duration> {
    let s = s.trim();
    if let Some(days) = s.strip_suffix('d').or_else(|| s.strip_suffix('D')) {
        let n: u64 = days.trim().parse().ok().filter(|n| *n > 0)?;
        return Some(std::time::Duration::from_secs(n * 86_400));
    }
    crate::parser::parse_wait_duration(s)
}

/// Definition-time rules for an expert node.
pub fn validate(activity: &Activity) -> Result<(), String> {
    if param_str(activity, "expert").trim().is_empty() {
        return Err(format!(
            "expert activity '{}' requires params.expert (a coworker's id, handle or name)",
            activity.id
        ));
    }
    if param_str(activity, "task").trim().is_empty() && activity.intent.trim().is_empty() {
        return Err(format!("expert activity '{}' requires params.task (what the expert should do)", activity.id));
    }
    if parse_timeout(param_str(activity, "timeout")).is_none() {
        return Err(format!(
            "expert activity '{}' requires params.timeout (e.g. \"30m\", \"4h\", \"2d\")",
            activity.id
        ));
    }
    if let Some(input) = activity.params.as_ref().and_then(|p| p.get("input")) {
        let refs: Vec<&Value> = match input {
            Value::Object(m) => m.values().collect(),
            Value::Array(a) => a.iter().collect(),
            other => vec![other],
        };
        for v in refs {
            let ok = v.as_str().is_some_and(|s| {
                let s = s.trim();
                s.starts_with("{{") && s.ends_with("}}") && s.matches("{{").count() == 1
            });
            if !ok {
                return Err(format!(
                    "expert activity '{}': every params.input value must be one explicit reference such as \"{{{{nodes.<id>}}}}\"",
                    activity.id
                ));
            }
        }
    }
    Ok(())
}

/// The data the expert is handed: each `params.input` reference resolved to
/// its value (whole JSON, not text), keyed as the author keyed it.
pub fn resolve_input(activity: &Activity, resolve: &dyn Fn(&str) -> Option<Value>) -> Value {
    let one = |v: &Value| -> Value {
        let path = v.as_str().unwrap_or("").trim().trim_start_matches("{{").trim_end_matches("}}").trim().to_string();
        resolve(&path).unwrap_or(Value::Null)
    };
    match activity.params.as_ref().and_then(|p| p.get("input")) {
        Some(Value::Object(m)) => Value::Object(m.iter().map(|(k, v)| (k.clone(), one(v))).collect()),
        Some(Value::Array(a)) => Value::Array(a.iter().map(one).collect()),
        Some(v @ Value::String(_)) => one(v),
        _ => Value::Null,
    }
}

/// `params.output`: a JSON shape or a short description of what comes back.
pub fn output_contract(activity: &Activity) -> String {
    match activity.params.as_ref().and_then(|p| p.get("output")) {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Null) | None => String::new(),
        Some(shape) => shape.to_string(),
    }
}

/// Who the expert is, once resolved.
#[derive(Debug, Clone)]
pub enum Expert {
    /// A coworker on this bot: agent id and name.
    Coworker { id: String, name: String },
}

/// Why a request could not be sent.
#[derive(Debug)]
pub enum Unsent {
    /// The expert does not exist or cannot take work: the step fails.
    Unreachable(String),
    /// The owner must decide first: the step is blocked.
    NeedsOwner(String),
}

/// Resolve `expert` for the employee that owns the workflow.
pub fn resolve(store: &Store, owner_agent_id: &str, expert: &str) -> Result<Expert, Unsent> {
    let expert = expert.trim();
    if let Some(addr) = expert.strip_prefix(LOOP_PREFIX) {
        return Err(Unsent::NeedsOwner(format!(
            "This step would send the workflow's data to {addr}, an agent on another bot. Data leaves \
             your bot only with your approval for this workflow and that expert, and Nebo has no way \
             to record that approval yet, so nothing was sent. Use a coworker on this bot instead."
        )));
    }
    if owner_agent_id.is_empty() {
        return Err(Unsent::Unreachable("an expert step runs only in an employee's workflow".into()));
    }
    let Some(agent) = tools::team::resolve_agent(store, expert) else {
        return Err(Unsent::Unreachable(format!("expert '{expert}' is not a coworker on this bot")));
    };
    if agent.is_enabled == 0 {
        return Err(Unsent::Unreachable(format!("expert '{}' is paused, so it cannot take the work", agent.name)));
    }
    if agent.id == owner_agent_id {
        return Err(Unsent::Unreachable(format!(
            "expert '{}' is the employee running this workflow; an expert step goes to someone else",
            agent.name
        )));
    }
    Ok(Expert::Coworker { id: agent.id, name: agent.name })
}

/// One request, as the step sends it.
pub struct Request<'a> {
    pub run_id: &'a str,
    pub key: &'a str,
    pub owner_agent_id: &'a str,
    pub workflow_name: &'a str,
    pub task: &'a str,
    pub input: &'a Value,
    pub contract: &'a str,
    pub timeout_secs: u64,
}

/// Where a request stands.
#[derive(Debug, Clone, PartialEq)]
pub enum Standing {
    /// The expert answered: the node output (always with `summary`).
    Answered(Value),
    /// The expert refused, could not do it, or did not answer in time.
    Failed(String),
    /// Sent and not answered yet; wake the run by this time at the latest.
    Waiting { deadline: i64 },
}

/// The request's standing from what is on record, or None when it was never
/// sent.
pub fn standing(store: &Store, run_id: &str, key: &str, timeout_secs: u64, now: i64) -> Option<Standing> {
    if let Some(reply) = recorded_reply(store, run_id, key) {
        return Some(read_reply(&reply));
    }
    let effect = store
        .engine_effects_for_run(run_id)
        .ok()?
        .into_iter()
        .find(|e| e.idem_key == effect_key(key))?;
    let deadline = effect.created_at + timeout_secs as i64;
    Some(if now >= deadline {
        Standing::Failed(format!("the expert did not answer within {}", human_secs(timeout_secs)))
    } else {
        Standing::Waiting { deadline }
    })
}

/// Send the request once. The ledger row is written first: a request is on
/// record before it can be in anyone's hands, so a re-entered run never sends
/// it twice.
pub fn send(store: &Store, expert: &Expert, req: &Request<'_>, now: i64) -> Result<Standing, Unsent> {
    let Expert::Coworker { id, name } = expert;
    let effect = store
        .engine_effect_pending(req.run_id, EFFECT_CLASS, &effect_key(req.key), "coworker", "", id)
        .map_err(|e| Unsent::Unreachable(format!("the request could not be recorded: {e}")))?;
    let owner_name = store
        .get_agent(req.owner_agent_id)
        .ok()
        .flatten()
        .map(|a| a.name)
        .unwrap_or_else(|| "a workflow".to_string());
    let subject = format!("{} (from the workflow '{}')", req.task.trim(), req.workflow_name);
    let done_means = done_means(req);
    let opened = crate::cases::open_assignment(
        store,
        &crate::cases::NewAssignmentRequest {
            assigner_agent_id: req.owner_agent_id,
            assigner_name: &owner_name,
            // The workflow is told through its wait, not a chat session.
            assigner_session_key: "",
            parent_run_id: Some(req.run_id),
            assignee_agent_id: id,
            subject: &subject,
            done_means: &done_means,
            due: None,
            workflow_reply: Some(crate::cases::WorkflowReply { run_id: req.run_id, key: req.key }),
        },
        now,
    );
    match opened {
        Ok(assignment_id) => {
            let _ = store.engine_effect_completed(effect, Some(&assignment_id), None, now);
            tracing::info!(run = req.run_id, key = req.key, expert = %name, assignment = %assignment_id, "expert request sent");
            Ok(Standing::Waiting { deadline: now + req.timeout_secs as i64 })
        }
        Err(e) => {
            let reason = format!("expert '{name}' could not be given the work: {e}");
            let _ = store.engine_effect_failed(effect, &reason, now);
            Err(Unsent::Unreachable(reason))
        }
    }
}

/// What done means for the expert: the task's data, the contract, and that
/// its closing summary is its reply.
fn done_means(req: &Request<'_>) -> String {
    let mut s = String::from(
        "Your closing summary is your reply to the workflow that asked: begin it with one short \
         sentence saying what you did or found.",
    );
    if !req.contract.is_empty() {
        s.push_str(&format!(" Then give the result as: {}.", req.contract));
    }
    if !req.input.is_null() {
        let data = req.input.to_string();
        let data = if data.len() > 8_000 { format!("{}…", truncate(&data, 8_000)) } else { data };
        s.push_str(&format!(" The data for the task: {data}"));
    }
    s.push_str(" If you cannot or will not do it, close as blocked or failed and say why.");
    s
}

/// Record an expert's answer to a workflow's request — the event that wakes
/// the run if it is parked on it. Called when the assignment's case closes.
/// One reply per request: a repeat is a duplicate and wakes nothing.
pub fn record_reply(store: &Store, run_id: &str, key: &str, state: &str, summary: &str, expert: &str) -> Result<(), types::NeboError> {
    let payload = serde_json::json!({ "state": state, "summary": summary, "expert": expert });
    store.engine_enqueue_event(&NewEvent {
        kind: REPLY_KIND,
        target_type: "run",
        target_id: &run_target(run_id),
        payload: &payload.to_string(),
        channel: "expert",
        r#ref: key,
        idem_key: &reply_idem(key),
        durable: true,
        ..Default::default()
    })?;
    Ok(())
}

fn recorded_reply(store: &Store, run_id: &str, key: &str) -> Option<db::EngineEvent> {
    let idem = reply_idem(key);
    store
        .engine_events_for("run", &run_target(run_id), 10_000)
        .ok()?
        .into_iter()
        .find(|e| e.idem_key == idem)
}

/// The id of a reply already on record for any of `keys`: a reply that came
/// while the run was busy wakes nothing, so the run checks before it parks.
pub fn any_reply(store: &Store, run_id: &str, keys: &[String]) -> Option<i64> {
    let idems: std::collections::HashSet<String> = keys.iter().map(|k| reply_idem(k)).collect();
    store
        .engine_events_for("run", &run_target(run_id), 10_000)
        .ok()?
        .into_iter()
        .find(|e| idems.contains(&e.idem_key))
        .map(|e| e.id)
}

fn read_reply(event: &db::EngineEvent) -> Standing {
    let p: Value = serde_json::from_str(&event.payload).unwrap_or_default();
    let summary = p["summary"].as_str().unwrap_or("").trim().to_string();
    let expert = p["expert"].as_str().unwrap_or("").to_string();
    match p["state"].as_str().unwrap_or("done") {
        "done" => Standing::Answered(answer(&expert, &summary)),
        state => Standing::Failed(if summary.is_empty() {
            format!("the expert closed the work as {state}")
        } else {
            format!("the expert closed the work as {state}: {summary}")
        }),
    }
}

/// The node output for an answer: `summary` (short, always), `reply` (the
/// result — JSON when the expert gave JSON, else its text) and `expert`.
pub fn answer(expert: &str, text: &str) -> Value {
    let (prose, json) = split_json(text);
    let summary = json
        .as_ref()
        .and_then(|j| j.get("summary"))
        .and_then(|s| s.as_str())
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| first_sentence(if prose.trim().is_empty() { text } else { &prose }));
    serde_json::json!({
        "summary": truncate(summary.trim(), SUMMARY_MAX),
        "reply": json.unwrap_or_else(|| Value::String(text.to_string())),
        "expert": expert,
    })
}

/// The node output for a failure the run goes on past.
pub fn failure(expert: &str, reason: &str) -> Value {
    serde_json::json!({
        "failed": true,
        "reason": reason,
        "summary": truncate(&format!("{expert} did not do it: {reason}"), SUMMARY_MAX),
        "expert": expert,
    })
}

/// Prose before a trailing JSON value, and that value.
fn split_json(text: &str) -> (String, Option<Value>) {
    let t = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(t)
        && (v.is_object() || v.is_array())
    {
        return (String::new(), Some(v));
    }
    for (i, c) in t.char_indices() {
        if (c == '{' || c == '[')
            && let Ok(v) = serde_json::from_str::<Value>(&t[i..])
        {
            return (t[..i].trim().to_string(), Some(v));
        }
    }
    (t.to_string(), None)
}

fn first_sentence(s: &str) -> String {
    let s = s.trim();
    match s.find(|c| matches!(c, '.' | '!' | '?' | '\n')) {
        Some(i) => s[..=i].trim().to_string(),
        None => s.to_string(),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

fn human_secs(secs: u64) -> String {
    match secs {
        s if s % 86_400 == 0 => format!("{}d", s / 86_400),
        s if s % 3_600 == 0 => format!("{}h", s / 3_600),
        s if s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

/// One expert the authoring path may name.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CatalogEntry {
    /// What goes in `params.expert`.
    pub expert: String,
    pub name: String,
    /// One line: what it is for.
    pub capability: String,
    pub department: String,
    pub skills: Vec<String>,
}

/// The experts a workflow of `owner_agent_id` can name: every enabled
/// coworker on this bot (not the owner itself), with its role and skills.
/// Agents on other bots are not listed: no workflow can reach them until
/// the owner can approve data leaving his bot for one.
pub fn catalog(store: &Store, owner_agent_id: &str) -> Vec<CatalogEntry> {
    let mut out: Vec<CatalogEntry> = store
        .list_agents(500, 0)
        .unwrap_or_default()
        .into_iter()
        .filter(|a| a.is_enabled != 0 && a.id != owner_agent_id && a.is_app.unwrap_or(0) == 0)
        .map(|a| {
            let skills = napp::agent::parse_agent_config(&a.frontmatter).map(|c| c.skills).unwrap_or_default();
            CatalogEntry {
                expert: a.handle.clone().filter(|h| !h.is_empty()).unwrap_or_else(|| a.id.clone()),
                capability: truncate(&first_sentence(&a.description), 160),
                department: a.department.clone().unwrap_or_default(),
                name: a.name,
                skills,
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// The catalog as lines for a prompt.
pub fn catalog_lines(entries: &[CatalogEntry]) -> String {
    entries
        .iter()
        .map(|e| {
            let mut line = format!("- {} (expert: \"{}\")", e.name, e.expert);
            if !e.capability.is_empty() {
                line.push_str(&format!(" — {}", e.capability));
            }
            if !e.skills.is_empty() {
                line.push_str(&format!(" [skills: {}]", e.skills.join(", ")));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeouts_take_days_and_refuse_nothing() {
        assert_eq!(parse_timeout("2d"), Some(std::time::Duration::from_secs(172_800)));
        assert_eq!(parse_timeout("30m"), Some(std::time::Duration::from_secs(1_800)));
        assert_eq!(parse_timeout(""), None);
        assert_eq!(parse_timeout("0d"), None);
    }

    #[test]
    fn an_answer_always_carries_a_short_summary() {
        let a = answer("Ana", "Priced all three. {\"total\": 1200}");
        assert_eq!(a["summary"], "Priced all three.");
        assert_eq!(a["reply"]["total"], 1200);
        let b = answer("Ana", "{\"summary\": \"ok\", \"rows\": [1]}");
        assert_eq!(b["summary"], "ok");
        let c = answer("Ana", &"word ".repeat(200));
        assert!(c["summary"].as_str().unwrap().len() <= SUMMARY_MAX + 3);
    }
}
