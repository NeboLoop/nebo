//! Work items and runs: `create_task`, `update_task`, `get_task`,
//! `list_tasks` track the steps of a long job in this conversation;
//! `assign_task` and `list_assignments` hand work to another employee as
//! their own; `list_runs` shows the work running now.

use std::sync::Arc;

use db::Store;
use serde_json::{Value, json};

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};
use crate::run_querier::RunQuerierHandle;

/// Statuses `update_task` sets. `deleted` takes a task off the list: its row
/// is marked skipped. There is no `failed`: finished work with a bad outcome
/// is completed with the failure in `output`; unfinished work stays
/// in_progress with a follow-up task.
const STATUSES: [&str; 4] = ["pending", "in_progress", "completed", "deleted"];

/// What `deleted` writes.
const SKIPPED: &str = "skipped";

pub struct Tasks {
    store: Arc<Store>,
    runs: RunQuerierHandle,
}

impl Tasks {
    pub fn new(store: Arc<Store>, runs: RunQuerierHandle) -> Self {
        Self { store, runs }
    }

    pub fn tools(self) -> Vec<Box<dyn DynTool>> {
        let tasks = Arc::new(self);
        TaskOp::ALL
            .into_iter()
            .map(|op| {
                Box::new(TaskTool {
                    op,
                    tasks: tasks.clone(),
                }) as Box<dyn DynTool>
            })
            .collect()
    }

    /// Items live on the conversation's own list.
    fn list_id(ctx: &ToolContext) -> String {
        format!("session:{}", ctx.session_id)
    }

    fn create(&self, input: &Value, ctx: &ToolContext) -> ToolResult {
        let subject = input["subject"].as_str().unwrap_or("");
        match self.store.create_task_item(
            &Self::list_id(ctx),
            subject,
            input["description"].as_str(),
        ) {
            Ok(task) => ToolResult::ok(format!("Task {} created: {subject}", task.id)),
            Err(e) => ToolResult::error(format!("Failed to create task: {e}")),
        }
    }

    fn update(&self, input: &Value) -> ToolResult {
        let task_id = input["task_id"].as_str().unwrap_or("");
        let status = input["status"].as_str().unwrap_or("");
        if status == "deleted" {
            return match self
                .store
                .update_task_item(task_id, SKIPPED, None, None, 0, 0)
            {
                Ok(_) => ToolResult::ok(format!(
                    "Task {task_id} taken off the list (it stays in the list with status skipped)."
                )),
                Err(e) => ToolResult::error(format!("Failed to update task: {e}")),
            };
        }
        match self
            .store
            .update_task_item(task_id, status, input["output"].as_str(), None, 0, 0)
        {
            Ok(_) => ToolResult::ok(format!("Task {task_id} updated to {status}")),
            Err(e) => ToolResult::error(format!("Failed to update task: {e}")),
        }
    }

    fn get(&self, input: &Value) -> ToolResult {
        let task_id = input["task_id"].as_str().unwrap_or("");
        match self.store.get_pending_task(task_id) {
            Ok(Some(t)) => {
                let desc = t.description.as_deref().unwrap_or(&t.prompt);
                let mut out = format!("Task {}: {desc}\nStatus: {}\n", t.id, t.status);
                if let Some(ref output) = t.output {
                    out.push_str(&format!("Output: {output}\n"));
                }
                if let Some(ref error) = t.last_error {
                    out.push_str(&format!("Error: {error}\n"));
                }
                ToolResult::ok(out)
            }
            Ok(None) => ToolResult::error(format!(
                "Task {task_id} not found. list_tasks shows this conversation's tasks."
            )),
            Err(e) => ToolResult::error(format!("Failed to get task: {e}")),
        }
    }

    fn list(&self, ctx: &ToolContext) -> ToolResult {
        match self.store.list_task_items(&Self::list_id(ctx)) {
            Ok(tasks) if tasks.is_empty() => ToolResult::ok("No tasks."),
            Ok(tasks) => {
                let lines: Vec<String> = tasks
                    .iter()
                    .map(|t| {
                        let output_hint = if t.status == "completed" && t.output.is_some() {
                            " [has output]"
                        } else {
                            ""
                        };
                        format!(
                            "{} [{}] {}{output_hint}",
                            t.id,
                            t.status,
                            t.description.as_deref().unwrap_or(&t.prompt)
                        )
                    })
                    .collect();
                ToolResult::ok(format!("{} tasks:\n{}", tasks.len(), lines.join("\n")))
            }
            Err(e) => ToolResult::error(format!("Failed to list tasks: {e}")),
        }
    }

    /// An employee by id, exact name, or case-insensitive name.
    fn find_employee(&self, who: &str) -> Option<db::models::Agent> {
        if let Ok(Some(a)) = self.store.get_agent(who) {
            return Some(a);
        }
        if let Ok(Some(a)) = self.store.get_agent_by_name(who) {
            return Some(a);
        }
        let want = who.trim().to_lowercase();
        self.store
            .list_agents(500, 0)
            .unwrap_or_default()
            .into_iter()
            .find(|a| a.name.trim().to_lowercase() == want)
    }

    fn assign(&self, input: &Value, ctx: &ToolContext) -> ToolResult {
        let to = input["to"].as_str().map(str::trim).unwrap_or("");
        let subject = input["subject"].as_str().map(str::trim).unwrap_or("");
        // A team takes work through its lead, who hands steps to the others.
        let team = match self.find_employee(to) {
            Some(_) => None,
            None => crate::team::resolve_team(&self.store, to).ok(),
        };
        let assignee = match &team {
            None => self.find_employee(to),
            Some(t) => match crate::team::lead_of(t) {
                Some(lead) => self.store.get_agent(lead).ok().flatten(),
                None => {
                    return ToolResult::error(format!(
                        "The {} team has no lead, so it can't take work. Set one with update_team(team: \"{}\", lead: \"Employee Name\"), or assign it to a member.",
                        t.name, t.name
                    ));
                }
            },
        };
        let Some(assignee) = assignee else {
            return ToolResult::error(format!(
                "No employee or team named \"{to}\". Use an exact name from the roster."
            ));
        };
        // A temporary team is for one piece of work: it is claimed here,
        // atomically, before the work opens, so two assignments never both
        // land; the claim takes the lead's case id once the case exists.
        let reserved = format!("reserved:{}", uuid::Uuid::new_v4());
        let claimed = match &team {
            Some(t) => match self.store.claim_temporary_run(db::TemporaryKind::Team, "", &t.id, &reserved) {
                Ok(db::TemporaryClaim::AlreadyRan(_)) => {
                    return ToolResult::error(format!(
                        "The temporary {} team already has its one piece of work; it disbands when that is done. Assign this to a member, or make another team.",
                        t.name
                    ));
                }
                Ok(db::TemporaryClaim::Claimed) => Some(t.id.clone()),
                Ok(db::TemporaryClaim::NotTemporary) => None,
                Err(e) => return ToolResult::error(format!("Failed to assign: {e}")),
            },
            None => None,
        };
        let assigner_id = caller_entity(ctx);
        if assignee.id == assigner_id {
            return ToolResult::error(
                "That is you. An assignment is work for another employee; do your own work directly.",
            );
        }
        let assigner_name = self
            .store
            .get_agent(&assigner_id)
            .ok()
            .flatten()
            .map(|a| a.name)
            .unwrap_or_else(|| "the owner".to_string());
        let req = crate::assignments::AssignmentRequest {
            assigner_agent_id: assigner_id,
            assigner_name,
            assigner_session_key: ctx.session_key.clone(),
            parent_run_id: ctx.run_id.clone(),
            assignee_agent_id: assignee.id.clone(),
            subject: subject.to_string(),
            done_means: input["done_means"]
                .as_str()
                .map(str::trim)
                .unwrap_or("")
                .to_string(),
            due: input["due"]
                .as_str()
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map(String::from),
        };
        let release = || {
            if let Some(team_id) = &claimed {
                let _ = self.store.release_temporary_run(db::TemporaryKind::Team, "", team_id, &reserved);
            }
        };
        let Some(opener) = crate::assignments::assignment_opener() else {
            release();
            return ToolResult::error(
                "Assignments are not ready: the server has not installed the opener yet. Try again in a moment.",
            );
        };
        match opener.open(&req) {
            Ok(id) => {
                let who = match &team {
                    Some(t) => {
                        // The temporary team's one piece of work: its case.
                        if let (Some(team_id), Ok(Some(case))) = (&claimed, self.store.engine_run_for_key("case:assignment", &id)) {
                            let _ = self.store.settle_temporary_run(db::TemporaryKind::Team, "", team_id, &reserved, &case.id);
                        }
                        let label = if t.name.to_lowercase().contains("team") { t.name.clone() } else { format!("{} team", t.name) };
                        format!("the {label} (its lead, {})", assignee.name)
                    }
                    None => assignee.name.clone(),
                };
                ToolResult::ok(format!(
                    "Assigned to {who} as their own work (assignment {id}). You will hear assignment.done, \
                     assignment.blocked, or assignment.failed when it closes; until then it is theirs — \
                     do not do it yourself."
                ))
            }
            Err(e) => {
                release();
                ToolResult::error(format!("Failed to assign: {e}"))
            }
        }
    }

    fn assignments(&self, input: &Value, ctx: &ToolContext) -> ToolResult {
        let me = caller_entity(ctx);
        let all = input["all"].as_bool().unwrap_or(false);
        match self.store.list_assignments_for_agent(&me, !all) {
            Ok(list) if list.is_empty() => ToolResult::ok("No assignments."),
            Ok(list) => {
                let name_of = |id: &str| -> String {
                    self.store
                        .get_agent(id)
                        .ok()
                        .flatten()
                        .map(|a| a.name)
                        .unwrap_or_else(|| id.to_string())
                };
                let lines: Vec<String> = list
                    .iter()
                    .map(|a| {
                        let dir = if a.assignee_agent_id == me {
                            format!("from {}", name_of(&a.assigner_agent_id))
                        } else {
                            format!("to {}", name_of(&a.assignee_agent_id))
                        };
                        format!(
                            "{} [{}] {} ({dir}){}{}",
                            a.id,
                            a.state,
                            a.subject,
                            a.due
                                .as_deref()
                                .map(|d| format!(", due {d}"))
                                .unwrap_or_default(),
                            a.outcome
                                .as_deref()
                                .filter(|_| a.state != "open")
                                .map(|o| format!(" → {o}"))
                                .unwrap_or_default()
                        )
                    })
                    .collect();
                ToolResult::ok(format!(
                    "{} assignment(s):\n{}",
                    list.len(),
                    lines.join("\n")
                ))
            }
            Err(e) => ToolResult::error(format!("Failed to list assignments: {e}")),
        }
    }

    /// The main employee sees every run; any other employee sees its own.
    async fn runs(&self, ctx: &ToolContext) -> ToolResult {
        let Some(querier) = self.runs.get() else {
            return ToolResult::error(
                "The run registry is not ready. Retry once after 5 seconds; if it fails again, do not retry.",
            );
        };
        let runs = querier.list_runs(&caller_entity(ctx)).await;
        if runs.is_empty() {
            return ToolResult::ok("No runs are active.");
        }
        let lines: Vec<String> = runs
            .iter()
            .map(|r| {
                let tool_info = if r.current_tool.is_empty() {
                    String::new()
                } else {
                    format!(" — running: {}", r.current_tool)
                };
                format!(
                    "- [{}] {} ({}) · {} tools · {}s{tool_info}",
                    r.run_id, r.entity_name, r.origin, r.tool_call_count, r.elapsed_secs
                )
            })
            .collect();
        ToolResult::ok(format!("{} active runs:\n{}", runs.len(), lines.join("\n")))
    }
}

/// The employee a call comes from: `agent:<id>:…` keys name it, anything
/// else is the main employee.
fn caller_entity(ctx: &ToolContext) -> String {
    let id = types::keyparser::extract_agent_id(&ctx.session_key);
    if id.is_empty() {
        "main".to_string()
    } else {
        id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskOp {
    Create,
    Update,
    Get,
    List,
    Assign,
    Assignments,
    Runs,
}

impl TaskOp {
    const ALL: [TaskOp; 7] = [
        TaskOp::Create,
        TaskOp::Update,
        TaskOp::Get,
        TaskOp::List,
        TaskOp::Assign,
        TaskOp::Assignments,
        TaskOp::Runs,
    ];
}

struct TaskTool {
    op: TaskOp,
    tasks: Arc<Tasks>,
}

impl DynTool for TaskTool {
    fn name(&self) -> &str {
        match self.op {
            TaskOp::Create => "create_task",
            TaskOp::Update => "update_task",
            TaskOp::Get => "get_task",
            TaskOp::List => "list_tasks",
            TaskOp::Assign => "assign_task",
            TaskOp::Assignments => "list_assignments",
            TaskOp::Runs => "list_runs",
        }
    }

    fn description(&self) -> String {
        match self.op {
            TaskOp::Create => "Adds a step to this conversation's task list.\n\
                 - Only for work that spans many tool calls in several distinct stages. Not for a small job, a single request or a quick fix.\n\
                 - Mark it in_progress when you start and completed when it's done, with update_task."
                .to_string(),
            TaskOp::Update => "Changes a task's status: pending, in_progress, completed, or deleted to take it off the list.\n\
                 - There is no failed: finished work with a bad outcome is completed with the failure in `output`; unfinished work stays in_progress with a follow-up task."
                .to_string(),
            TaskOp::Get => "Reads one task: its status, output and any error.".to_string(),
            TaskOp::List => "Lists this conversation's tasks with their status.".to_string(),
            TaskOp::Assign => "Gives a piece of work to another employee, or to a team through its lead, as their own work, not a helper of yours.\n\
                 - They work it as a case; you're told assignment.done, blocked or failed when it closes. Until then it's theirs: don't do it yourself.\n\
                 - `done_means` says what finished looks like; `due` is a date."
                .to_string(),
            TaskOp::Assignments => "Lists your open assignments, given and received. `all: true` includes closed ones.".to_string(),
            TaskOp::Runs => "Lists the work running now: employee runs, their origin, tool calls and elapsed time. Stop one with stop_task.".to_string(),
        }
    }

    fn schema(&self) -> Value {
        match self.op {
            TaskOp::Create => json!({
                "type": "object",
                "properties": {
                    "subject": { "type": "string", "description": "The step, in a few words." },
                    "description": { "type": "string", "description": "What done looks like, if the subject doesn't say." }
                },
                "required": ["subject"]
            }),
            TaskOp::Update => json!({
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "The task's id." },
                    "status": { "type": "string", "enum": STATUSES, "description": "The new status; deleted takes it off the list." },
                    "output": { "type": "string", "description": "What the task produced, or what failed and why." }
                },
                "required": ["task_id", "status"]
            }),
            TaskOp::Get => json!({
                "type": "object",
                "properties": { "task_id": { "type": "string", "description": "The task's id." } },
                "required": ["task_id"]
            }),
            TaskOp::List | TaskOp::Runs => json!({ "type": "object", "properties": {} }),
            TaskOp::Assign => json!({
                "type": "object",
                "properties": {
                    "to": { "type": "string", "description": "An employee's name from the roster, or a team's: its lead takes it." },
                    "subject": { "type": "string", "description": "The work, in a sentence." },
                    "done_means": { "type": "string", "description": "What finished looks like." },
                    "due": { "type": "string", "description": "Due date, YYYY-MM-DD." }
                },
                "required": ["to", "subject"]
            }),
            TaskOp::Assignments => json!({
                "type": "object",
                "properties": { "all": { "type": "boolean", "description": "Include closed assignments." } }
            }),
        }
    }

    fn search_hint(&self) -> &str {
        match self.op {
            TaskOp::Create => "track a step of long work",
            TaskOp::Update => "mark a task done or in progress",
            TaskOp::Get => "read one tracked task",
            TaskOp::List => "list this conversation's tasks",
            TaskOp::Assign => "give work to another employee",
            TaskOp::Assignments => "list assignments given and received",
            TaskOp::Runs => "list active runs and running work",
        }
    }

    fn read_only(&self, _input: &Value) -> bool {
        matches!(
            self.op,
            TaskOp::Get | TaskOp::List | TaskOp::Assignments | TaskOp::Runs
        )
    }

    /// Tracking steps is the employee's own work; handing work to a coworker
    /// is not.
    fn effects(&self, _input: &Value) -> types::permissions::CallEffects {
        if self.op == TaskOp::Assign {
            types::permissions::CallEffects::unknown()
        } else {
            types::permissions::CallEffects::none()
        }
    }

    fn validate_input(&self, input: &Value) -> Result<(), String> {
        let blank = |k: &str| input[k].as_str().is_none_or(|s| s.trim().is_empty());
        let required: &[&str] = match self.op {
            TaskOp::Create => &["subject"],
            TaskOp::Update | TaskOp::Get => &["task_id"],
            TaskOp::Assign => &["to", "subject"],
            _ => &[],
        };
        if let Some(k) = required.iter().find(|k| blank(k)) {
            return Err(format!("{k} can't be empty."));
        }
        if self.op == TaskOp::Update && input["status"].as_str() == Some("failed") {
            return Err(format!(
                "'failed' is not a task status. If the work is finished and the outcome was a \
                 failure, mark it completed with the failure in output: update_task(task_id: \"{}\", \
                 status: \"completed\", output: \"<what failed and why>\"). If it can still be \
                 finished, keep it in_progress and create a follow-up task.",
                input["task_id"].as_str().unwrap_or("")
            ));
        }
        Ok(())
    }

    fn activity(&self, input: &Value) -> String {
        match self.op {
            TaskOp::Create => format!("adding a task: {}", input["subject"].as_str().unwrap_or("")),
            TaskOp::Update => format!("marking a task {}", input["status"].as_str().unwrap_or("")),
            TaskOp::Get | TaskOp::List => "checking tasks".to_string(),
            TaskOp::Assign => format!(
                "assigning work to {}",
                input["to"].as_str().unwrap_or("a coworker")
            ),
            TaskOp::Assignments => "checking assignments".to_string(),
            TaskOp::Runs => "checking running work".to_string(),
        }
    }

    fn outcome(&self, input: &Value) -> String {
        match self.op {
            TaskOp::Create => format!("Added a task: {}", input["subject"].as_str().unwrap_or("")),
            TaskOp::Update => format!("Marked a task {}", input["status"].as_str().unwrap_or("")),
            TaskOp::Get | TaskOp::List => "Checked tasks".to_string(),
            TaskOp::Assign => format!(
                "Assigned work to {}",
                input["to"].as_str().unwrap_or("a coworker")
            ),
            TaskOp::Assignments => "Checked assignments".to_string(),
            TaskOp::Runs => "Checked running work".to_string(),
        }
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            match self.op {
                TaskOp::Create => self.tasks.create(&input, ctx),
                TaskOp::Update => self.tasks.update(&input),
                TaskOp::Get => self.tasks.get(&input),
                TaskOp::List => self.tasks.list(ctx),
                TaskOp::Assign => self.tasks.assign(&input, ctx),
                TaskOp::Assignments => self.tasks.assignments(&input, ctx),
                TaskOp::Runs => self.tasks.runs(ctx).await,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rig {
        tools: Vec<Box<dyn DynTool>>,
        store: Arc<Store>,
        _dir: tempfile::TempDir,
    }

    impl Rig {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let store = Arc::new(Store::new(&dir.path().join("t.db").to_string_lossy()).unwrap());
            let tools = Tasks::new(store.clone(), crate::run_querier::new_handle()).tools();
            Self {
                tools,
                store,
                _dir: dir,
            }
        }

        async fn call(&self, name: &str, input: Value) -> ToolResult {
            let ctx = ToolContext {
                session_id: "s1".into(),
                ..Default::default()
            };
            let tool = self.tools.iter().find(|t| t.name() == name).unwrap();
            match tool.validate_input(&input) {
                Err(e) => ToolResult::error(e),
                Ok(()) => tool.execute_dyn(&ctx, input).await,
            }
        }
    }

    #[tokio::test]
    async fn a_task_is_created_listed_updated_and_taken_off() {
        let rig = Rig::new();
        let created = rig
            .call("create_task", json!({"subject": "Draft the report"}))
            .await;
        assert!(
            created.content.contains("Draft the report"),
            "{}",
            created.content
        );
        let id = rig.store.list_task_items("session:s1").unwrap()[0]
            .id
            .clone();
        assert!(
            rig.call("list_tasks", json!({}))
                .await
                .content
                .contains("Draft the report")
        );
        let done = rig
            .call(
                "update_task",
                json!({"task_id": id, "status": "completed", "output": "sent"}),
            )
            .await;
        assert!(!done.is_error, "{}", done.content);
        let off = rig
            .call("update_task", json!({"task_id": id, "status": "deleted"}))
            .await;
        assert!(
            off.content.contains("status skipped") && !off.content.contains("deleted"),
            "{}",
            off.content
        );
        assert_eq!(
            rig.store.get_pending_task(&id).unwrap().unwrap().status,
            "skipped"
        );
    }

    /// The task rules say never to mark work failed: it is either finished,
    /// with the failure in output, or still open.
    #[tokio::test]
    async fn failed_is_refused_and_the_alternatives_named() {
        let rig = Rig::new();
        let item = rig.store.create_task_item("list-1", "Draft", None).unwrap();
        let r = rig
            .call(
                "update_task",
                json!({"task_id": item.id, "status": "failed", "output": "boom"}),
            )
            .await;
        assert!(
            r.is_error
                && r.content.contains("status: \"completed\", output:")
                && r.content.contains("keep it in_progress"),
            "{}",
            r.content
        );
        assert_eq!(
            rig.store
                .get_pending_task(&item.id)
                .unwrap()
                .unwrap()
                .status,
            "pending",
            "a refused update writes nothing"
        );
    }

    #[tokio::test]
    async fn assigning_to_nobody_names_the_roster_rule() {
        let rig = Rig::new();
        let r = rig
            .call(
                "assign_task",
                json!({"to": "Ghost", "subject": "Close the books"}),
            )
            .await;
        assert!(
            r.is_error && r.content.contains("No employee or team named \"Ghost\""),
            "{}",
            r.content
        );
    }

    #[tokio::test]
    async fn list_runs_without_a_registry_says_so() {
        let r = Rig::new().call("list_runs", json!({})).await;
        assert!(
            r.is_error && r.content.contains("not ready"),
            "{}",
            r.content
        );
    }
}
