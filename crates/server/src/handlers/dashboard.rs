use std::collections::{HashMap, HashSet};

use axum::Json;
use axum::extract::State;
use types::api::{
    DashboardApproval, DashboardCounts, DashboardDay, DashboardEmployee, DashboardEmployeeRuns,
    DashboardResponse, DashboardRun,
};

use super::{HandlerResult, to_error_response};
use crate::state::AppState;

/// How far back the run charts look.
const HISTORY_DAYS: i64 = 14;
/// Rows in the recent-runs table.
const RECENT_RUNS: i64 = 12;
/// A workflow run older than this with no completion is treated as stopped:
/// the process that ran it is gone and nothing will finish it.
const ABANDONED_RUN_SECS: i64 = 6 * 60 * 60;

/// GET /api/v1/dashboard. The whole workforce on one page, composed from
/// what the server already tracks: the run registry (what is working now),
/// the pending approvals (what waits on the owner), the workflow run table
/// and the chat history (what happened), and the schedules (what is next).
pub async fn dashboard(State(state): State<AppState>) -> HandlerResult<DashboardResponse> {
    let now = chrono::Local::now();
    let now_ts = now.timestamp();
    let today_start = now.date_naive().and_hms_opt(0, 0, 0).map(|d| d.and_local_timezone(chrono::Local).single()).flatten().map(|d| d.timestamp()).unwrap_or(now_ts - 86_400);
    let history_start = today_start - (HISTORY_DAYS - 1) * 86_400;

    let agents = state.store.list_agents(1000, 0).map_err(to_error_response)?;
    let active_ids: HashSet<String> = state.agent_registry.read().await.keys().cloned().collect();
    let running = state.run_registry.list_all().await;
    let tool_approvals: Vec<(String, crate::state::PendingToolApproval)> = state
        .pending_tool_approvals
        .lock()
        .await
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let parked = state.store.list_workflow_suspensions().unwrap_or_default();
    let runs = state
        .store
        .list_workflow_runs_since(history_start, 2000)
        .map_err(to_error_response)?;
    let by_day = state
        .store
        .count_workflow_runs_by_day(history_start)
        .map_err(to_error_response)?;
    let chat_turns = state
        .store
        .count_chat_turns_by_day(history_start)
        .map_err(to_error_response)?;

    let name_of: HashMap<String, String> = agents.iter().map(|a| (a.id.clone(), a.name.clone())).collect();
    let name = |id: &str| name_of.get(id).cloned().unwrap_or_else(|| id.to_string());

    // ---- approvals: gated tool calls in chats, workflows parked at a step
    let mut approvals: Vec<DashboardApproval> = tool_approvals
        .iter()
        .map(|(id, a)| DashboardApproval {
            id: id.clone(),
            kind: "tool".into(),
            agent_id: a.agent_id.clone(),
            agent_name: name(&a.agent_id),
            summary: a.summary.clone(),
            since: a.since,
            chat_id: Some(types::keyparser::parse_session_key(&a.session_key).chat_id).filter(|t| !t.is_empty()),
        })
        .collect();
    for (run_id, agent_id, binding, display, created_at) in &parked {
        approvals.push(DashboardApproval {
            id: run_id.clone(),
            kind: "workflow".into(),
            agent_id: agent_id.clone(),
            agent_name: name(agent_id),
            summary: if display.is_empty() { binding.clone() } else { display.clone() },
            since: *created_at,
            chat_id: None,
        });
    }
    approvals.sort_by_key(|a| a.since);
    let waiting_ids: HashSet<&str> = approvals.iter().map(|a| a.agent_id.as_str()).collect();

    // ---- per employee: the workflow run it is on now, and the last one that ended
    // (`runs` is newest first).
    let mut live_wf: HashMap<&str, &db::models::WorkflowRun> = HashMap::new();
    let mut last_wf: HashMap<&str, &db::models::WorkflowRun> = HashMap::new();
    for r in &runs {
        let Some(agent_id) = types::keyparser::agent_id_from_workflow_id(&r.workflow_id) else { continue };
        if outcome_of(&r.status, r.started_at, now_ts) == "working" {
            live_wf.entry(agent_id).or_insert(r);
        } else {
            last_wf.entry(agent_id).or_insert(r);
        }
    }
    // Where a live run is in its workflow, when the workflow declares steps.
    let step_of = |r: &db::models::WorkflowRun| -> (Option<String>, Option<u32>, Option<u32>) {
        let Some(wf) = state.store.get_workflow(&r.workflow_id).ok().flatten() else { return (None, None, None) };
        let Ok(def) = state.workflow_manager.load_workflow_def(&wf) else { return (None, None, None) };
        let idx = r.current_activity.as_ref().and_then(|cur| def.activities.iter().position(|a| &a.id == cur));
        (Some(def.name).filter(|n| !n.is_empty()), idx.map(|i| i as u32 + 1), Some(def.activities.len() as u32))
    };

    // ---- employees
    let mut working_ids: HashSet<String> = HashSet::new();
    let mut employees = Vec::with_capacity(agents.len());
    for a in &agents {
        // "main" is the registry's name for the primary employee's own runs.
        let live: Vec<_> = running
            .iter()
            .filter(|r| r.entity_id == a.id || (r.entity_id == "main" && a.id == "assistant"))
            .collect();
        // A working card points at the chat the live run is on, not the most
        // recently touched chat: "Open chat" must land where things are moving.
        let live_chat = live.first().and_then(|run| {
            let session = state.store.get_session_by_name(&run.session_key).ok().flatten()?;
            state.store.get_chat(session.active_chat_id.as_deref()?).ok().flatten()
        });
        let latest = live_chat.or_else(|| state.store.get_latest_agent_chat(&a.id).ok().flatten());
        let latest_title = latest.as_ref().and_then(|c| display_title(&c.title));
        let isolated = crate::workflow_manager::agent_context_isolated(&state.store, &a.id);
        let matters = state.store.count_agent_chats(&a.id).unwrap_or(0) as u32;
        let paused = !active_ids.contains(&a.id);
        let wf_now = live_wf.get(a.id.as_str()).copied();
        let (wf_name, step, step_count) = wf_now.map(step_of).unwrap_or((None, None, None));
        let (status, task, activity, tool_calls, elapsed_secs) = if let Some(run) = live.first() {
            working_ids.insert(a.id.clone());
            let doing = if run.current_tool.is_empty() { "thinking".to_string() } else { format!("using {}", run.current_tool) };
            (
                "working",
                wf_name.clone().or_else(|| latest_title.clone()).unwrap_or_else(|| format!("Working over {}", run.channel)),
                format!("{doing}, {}", elapsed_phrase(run.elapsed_secs)),
                run.tool_call_count,
                run.elapsed_secs,
            )
        } else if waiting_ids.contains(a.id.as_str()) {
            let what = approvals.iter().find(|p| p.agent_id == a.id).map(|p| p.summary.clone()).unwrap_or_default();
            ("waiting", what, "Waiting for your okay".to_string(), 0, 0)
        } else if paused {
            ("paused", "Paused".to_string(), "Schedules and triggers wait until you resume".to_string(), 0, 0)
        } else {
            let next = crate::handlers::agents::compute_next_fire(&state.store, &a.id, &now);
            let activity = match next {
                Some(ts) if ts > now_ts => format!("Next run {}", in_phrase(ts - now_ts)),
                _ => "Nothing scheduled".to_string(),
            };
            (
                "idle",
                latest_title.clone().unwrap_or_else(|| first_line(&a.description)),
                activity,
                0,
                0,
            )
        };
        employees.push(DashboardEmployee {
            id: a.id.clone(),
            name: a.name.clone(),
            color: a.color.clone(),
            status: status.into(),
            task,
            activity,
            last_activity_at: latest.as_ref().map(|c| c.updated_at),
            chat_id: latest.as_ref().map(|c| c.id.clone()),
            tool_calls,
            elapsed_secs,
            isolated,
            matters,
            run_id: wf_now.map(|r| r.id.clone()),
            step,
            step_count,
            last_outcome: last_wf.get(a.id.as_str()).map(|r| outcome_of(&r.status, r.started_at, now_ts).to_string()),
            last_detail: last_wf.get(a.id.as_str()).map(|r| run_detail(outcome_of(&r.status, r.started_at, now_ts), r)),
            last_run_at: last_wf.get(a.id.as_str()).map(|r| r.completed_at.unwrap_or(r.started_at)),
        });
    }
    // The sidebar's order, so a card never jumps when its employee starts or
    // stops: a working card is marked, not moved.
    let app_ids: HashSet<&str> = agents.iter().filter(|a| a.is_app.unwrap_or(0) != 0).map(|a| a.id.as_str()).collect();
    employees.sort_by(|x, y| {
        card_rank(&x.id, app_ids.contains(x.id.as_str()))
            .cmp(&card_rank(&y.id, app_ids.contains(y.id.as_str())))
            .then_with(|| x.name.cmp(&y.name))
    });

    // ---- history: workflow runs by day and by employee, chat turns by day
    let mut days: HashMap<String, DashboardDay> = HashMap::new();
    let mut per_employee: HashMap<String, u32> = HashMap::new();
    for (day, workflow_id, status, count) in &by_day {
        let d = days.entry(day.clone()).or_insert_with(|| DashboardDay { day: day.clone(), done: 0, skipped: 0, stopped: 0, waiting: 0, chat_turns: 0 });
        let n = *count as u32;
        match outcome_of(status, 0, now_ts) {
            "done" => d.done += n,
            "skipped" => d.skipped += n,
            "waiting" => d.waiting += n,
            "working" => {}
            _ => d.stopped += n,
        }
        if let Some(agent_id) = types::keyparser::agent_id_from_workflow_id(workflow_id) {
            *per_employee.entry(agent_id.to_string()).or_default() += n;
        }
    }
    for (day, agent_id, count) in &chat_turns {
        let d = days.entry(day.clone()).or_insert_with(|| DashboardDay { day: day.clone(), done: 0, skipped: 0, stopped: 0, waiting: 0, chat_turns: 0 });
        d.chat_turns += *count as u32;
        *per_employee.entry(agent_id.clone()).or_default() += *count as u32;
    }
    let mut runs_by_day: Vec<DashboardDay> = (0..HISTORY_DAYS)
        .map(|i| {
            let day = (now.date_naive() - chrono::Duration::days(HISTORY_DAYS - 1 - i)).format("%Y-%m-%d").to_string();
            days.remove(&day).unwrap_or(DashboardDay { day: day.clone(), done: 0, skipped: 0, stopped: 0, waiting: 0, chat_turns: 0 })
        })
        .collect();
    runs_by_day.sort_by(|a, b| a.day.cmp(&b.day));
    let mut runs_by_employee: Vec<DashboardEmployeeRuns> = per_employee
        .into_iter()
        .map(|(agent_id, runs)| DashboardEmployeeRuns { agent_name: name(&agent_id), agent_id, runs })
        .collect();
    runs_by_employee.sort_by(|a, b| b.runs.cmp(&a.runs).then_with(|| a.agent_name.cmp(&b.agent_name)));

    // ---- today's counts
    let today_key = now.date_naive().format("%Y-%m-%d").to_string();
    let today = runs_by_day.iter().find(|d| d.day == today_key).cloned();
    let (done_today, skipped_today, stopped_today, chat_turns_today) =
        today.map(|d| (d.done, d.skipped, d.stopped, d.chat_turns)).unwrap_or((0, 0, 0, 0));
    let working_today = runs.iter().filter(|r| r.started_at >= today_start && outcome_of(&r.status, r.started_at, now_ts) == "working").count() as u32;
    let counts = DashboardCounts {
        employees: agents.len() as u32,
        working: working_ids.len() as u32,
        paused: employees.iter().filter(|e| e.status == "paused").count() as u32,
        waiting: approvals.len() as u32,
        runs_today: done_today + skipped_today + stopped_today + working_today + chat_turns_today,
        done_today,
        skipped_today,
        stopped_today,
        chat_turns_today,
    };

    // ---- recent runs: live runs first, then the newest workflow runs
    let mut recent_runs: Vec<DashboardRun> = running
        .iter()
        .map(|r| {
            let agent_id = if r.entity_id == "main" { "assistant".to_string() } else { r.entity_id.clone() };
            DashboardRun {
                id: r.run_id.clone(),
                agent_name: name(&agent_id),
                agent_id,
                title: if r.entity_name.is_empty() { format!("Run over {}", r.channel) } else { format!("{} over {}", r.entity_name, r.channel) },
                started_at: now_ts - r.elapsed_secs as i64,
                ended_at: None,
                outcome: "working".into(),
                detail: if r.current_tool.is_empty() { "Thinking".to_string() } else { format!("Using {}", r.current_tool) },
            }
        })
        .collect();
    for r in runs.iter().take(RECENT_RUNS as usize) {
        let agent_id = types::keyparser::agent_id_from_workflow_id(&r.workflow_id).unwrap_or("").to_string();
        let outcome = outcome_of(&r.status, r.started_at, now_ts);
        recent_runs.push(DashboardRun {
            id: r.id.clone(),
            agent_name: name(&agent_id),
            agent_id,
            title: r.trigger_detail.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| r.trigger_type.clone()),
            started_at: r.started_at,
            ended_at: r.completed_at,
            outcome: outcome.into(),
            detail: run_detail(outcome, r),
        });
    }

    Ok(Json(DashboardResponse { employees, counts, approvals, recent_runs, runs_by_day, runs_by_employee }))
}

/// The sidebar's order: the primary employee first, then everyone by name,
/// apps last. Status is deliberately not part of it.
fn card_rank(id: &str, is_app: bool) -> (bool, bool) {
    (id != "assistant", is_app)
}

/// What a run's row says about how it went, in the owner's words. The raw
/// message stays on the run itself, one tap deeper.
fn run_detail(outcome: &str, r: &db::models::WorkflowRun) -> String {
    match outcome {
        "done" => "Done".to_string(),
        "skipped" => match r.error.as_deref().map(owner_wording).filter(|e| !e.is_empty()) {
            Some(why) => format!("Nothing to do: {}", lower_first(&why)),
            None => "Nothing to do".to_string(),
        },
        "working" => r.current_activity.clone().unwrap_or_else(|| "Working".to_string()),
        "waiting" => "Waiting for your okay".to_string(),
        _ => r.error.as_deref().map(owner_wording).filter(|e| !e.is_empty()).map(|e| upper_first(&e)).unwrap_or_else(|| "Stopped".to_string()),
    }
}

/// A run's error or exit reason as the owner should read it: the evaluator's
/// "Step 2/8 evaluator:" bookkeeping dropped, a guard's counter turned into
/// what happened, everything else its first line.
fn owner_wording(raw: &str) -> String {
    let line = first_line(raw);
    let line = line.trim();
    if line.starts_with("activity run exceeded token budget") {
        return "Ran out of room and stopped".to_string();
    }
    // "Step 2/8 evaluator: No meetings ..." -> "No meetings ..."
    line.strip_prefix("Step ")
        .and_then(|rest| rest.split_once("evaluator:"))
        .map(|(_, why)| why.trim())
        .unwrap_or(line)
        .to_string()
}

fn upper_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn lower_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// A workflow run's status as a dashboard outcome.
fn outcome_of(status: &str, started_at: i64, now_ts: i64) -> &'static str {
    match status {
        "completed" => "done",
        "exited" => "skipped",
        "suspended" | "awaiting_approval" => "waiting",
        "running" | "pending" => {
            if started_at > 0 && now_ts - started_at > ABANDONED_RUN_SECS { "stopped" } else { "working" }
        }
        _ => "stopped",
    }
}

/// A thread title fit for a card: not empty, not a raw session key, not an
/// id. Titles are minted from the first message, so a thread that never got
/// one carries its key.
fn display_title(title: &str) -> Option<String> {
    let t = title.trim();
    let looks_like_key = t.starts_with("agent:") || t.contains(':') && t.len() > 40;
    let looks_like_id = t.len() == 36 && t.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    (!t.is_empty() && !looks_like_key && !looks_like_id).then(|| t.to_string())
}

fn elapsed_phrase(secs: u64) -> String {
    if secs < 90 { format!("{secs} s in") } else { format!("{} min in", secs / 60) }
}

fn in_phrase(secs: i64) -> String {
    if secs < 90 { "in under 2 minutes".to_string() }
    else if secs < 3600 { format!("in {} min", secs / 60) }
    else if secs < 86_400 { format!("in {} h", secs / 3600) }
    else { format!("in {} days", secs / 86_400) }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").chars().take(120).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_left_running_for_hours_counts_as_stopped() {
        let now = 1_000_000;
        assert_eq!(outcome_of("running", now - 60, now), "working");
        assert_eq!(outcome_of("running", now - ABANDONED_RUN_SECS - 1, now), "stopped");
        assert_eq!(outcome_of("completed", 0, now), "done");
        assert_eq!(outcome_of("exited", 0, now), "skipped");
        assert_eq!(outcome_of("failed", 0, now), "stopped");
        assert_eq!(outcome_of("suspended", 0, now), "waiting");
    }

    #[test]
    fn cards_keep_the_sidebar_order_whatever_their_status() {
        let mut ids = vec![("zed", false), ("app", true), ("assistant", false), ("amy", false)];
        ids.sort_by(|x, y| card_rank(x.0, x.1).cmp(&card_rank(y.0, y.1)).then_with(|| x.0.cmp(y.0)));
        let order: Vec<&str> = ids.iter().map(|x| x.0).collect();
        assert_eq!(order, ["assistant", "amy", "zed", "app"]);
    }

    fn sample_run(status: &str) -> db::models::WorkflowRun {
        db::models::WorkflowRun {
            id: "r1".into(),
            workflow_id: "agent:a1:wf".into(),
            trigger_type: "schedule".into(),
            trigger_detail: None,
            status: status.into(),
            inputs: None,
            current_activity: None,
            total_tokens_used: None,
            error: None,
            error_activity: None,
            session_key: None,
            output: None,
            started_at: 0,
            completed_at: None,
        }
    }

    #[test]
    fn exit_reasons_read_as_the_owner_would_say_them() {
        assert_eq!(owner_wording("Step 1/8 evaluator: No meetings with external guests in the next 24 hours"), "No meetings with external guests in the next 24 hours");
        assert_eq!(owner_wording("activity run exceeded token budget (4772/4096)"), "Ran out of room and stopped");
        assert_eq!(owner_wording("connection refused\nsecond line"), "connection refused");
        assert_eq!(owner_wording(""), "");
        let skipped = db::models::WorkflowRun { error: Some("Step 2/8 evaluator: No meetings found.".into()), ..sample_run("exited") };
        assert_eq!(run_detail("skipped", &skipped), "Nothing to do: no meetings found.");
        let stopped = db::models::WorkflowRun { error: Some("connection refused".into()), ..sample_run("failed") };
        assert_eq!(run_detail("stopped", &stopped), "Connection refused");
    }

    #[test]
    fn a_card_never_shows_a_session_key_as_a_title() {
        assert_eq!(display_title("Creating 01-tools Project").as_deref(), Some("Creating 01-tools Project"));
        assert_eq!(display_title("agent:5256dc2f-13f2-4955-863d-25b3857e5e1a:thread:9e1c"), None);
        assert_eq!(display_title("9e1c5f2a-1b2c-4d3e-8f90-123456789abc"), None);
        assert_eq!(display_title("  "), None);
    }
}
