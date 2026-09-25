//! The workflow tools: list, install, create, change, delete, run and read
//! the runs of workflows, one purpose each over the one [`WorkflowManager`].
//! Stopping a run is `stop_task`.

use std::sync::Arc;

use super::manager::WorkflowManager;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// One tool of the workflow family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    List,
    Install,
    Uninstall,
    Create,
    Update,
    Delete,
    Run,
    Status,
    Runs,
    SetEnabled,
}

const KINDS: &[Kind] = &[
    Kind::List,
    Kind::Install,
    Kind::Uninstall,
    Kind::Create,
    Kind::Update,
    Kind::Delete,
    Kind::Run,
    Kind::Status,
    Kind::Runs,
    Kind::SetEnabled,
];

/// Workflows belong to an employee; this is how every workflow tool says so.
const EMPLOYEE_NOTE: &str = "Workflows belong to an employee: yours by default; `employee` names another's (when the owner changes an employee's duties, the workflow goes on that employee, never on you).";

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::List => "list_workflows",
            Kind::Install => "install_workflow",
            Kind::Uninstall => "uninstall_workflow",
            Kind::Create => "create_workflow",
            Kind::Update => "update_workflow",
            Kind::Delete => "delete_workflow",
            Kind::Run => "run_workflow",
            Kind::Status => "workflow_status",
            Kind::Runs => "list_workflow_runs",
            Kind::SetEnabled => "set_workflow_enabled",
        }
    }

    fn search_hint(self) -> &'static str {
        match self {
            Kind::List => "list workflows and automations",
            Kind::Install => "install a workflow from a code",
            Kind::Uninstall => "uninstall a marketplace workflow",
            Kind::Create => "create an automated workflow",
            Kind::Update => "change an existing workflow",
            Kind::Delete => "delete a workflow",
            Kind::Run => "run a workflow now",
            Kind::Status => "latest run of a workflow",
            Kind::Runs => "recent runs of a workflow",
            Kind::SetEnabled => "turn a workflow on or off",
        }
    }

    fn description(self) -> String {
        match self {
            Kind::List => format!("Lists workflows and whether each is on. {EMPLOYEE_NOTE}"),
            Kind::Install => "Installs a workflow from a marketplace code (WORK-XXXX-XXXX).".to_string(),
            Kind::Uninstall => "Uninstalls a marketplace-installed workflow by its install id (from list_workflows), not its name.".to_string(),
            Kind::Create => format!(
                "Creates a workflow: automated work that runs on its trigger or on demand.\n\
                - `definition` is the workflow JSON: {{\"trigger\": {{\"type\": \"schedule\", \"cron\": \"0 9 * * MON-FRI\"}}, \"activities\": [{{\"id\": \"run\", \"intent\": \"what this accomplishes\", \"steps\": [\"concrete step\"]}}]}}. Leave out the trigger for a workflow run by hand.\n\
                - Activities are the only executable unit; each runs its intent and steps on its own. A top-level `steps` array is one activity.\n\
                - The name goes in `name`, or as \"name\" inside the definition.\n\
                - {EMPLOYEE_NOTE}"
            ),
            Kind::Update => format!(
                "Replaces an existing workflow's definition (same shape as create_workflow; not a partial patch). Its run history stays attached. {EMPLOYEE_NOTE}"
            ),
            Kind::Delete => format!("Deletes a workflow by name, with its trigger. {EMPLOYEE_NOTE}"),
            Kind::Run => format!(
                "Starts a workflow now, in the background, and returns its run id at once.\n\
                - `inputs` are the workflow's input values.\n\
                - Its outcome lands in its run history; don't poll workflow_status while it runs. To stop it, stop_task with the run id.\n\
                - {EMPLOYEE_NOTE}"
            ),
            Kind::Status => format!(
                "Shows a workflow's latest run: its state, and for a finished run what it did. Checking again while it runs won't speed it up. {EMPLOYEE_NOTE}"
            ),
            Kind::Runs => format!("Lists a workflow's ten most recent runs. {EMPLOYEE_NOTE}"),
            Kind::SetEnabled => format!(
                "Turns a workflow on (`enabled: true`) so its trigger fires, or off. {EMPLOYEE_NOTE}"
            ),
        }
    }

    fn schema(self) -> serde_json::Value {
        let employee = serde_json::json!({ "type": "string", "description": "The employee (name or id) whose workflows these are. Default: you." });
        let workflow = serde_json::json!({ "type": "string", "description": "The workflow's name or id." });
        let definition = serde_json::json!({ "type": "string", "description": "The workflow JSON." });
        match self {
            Kind::List => serde_json::json!({
                "type": "object",
                "properties": { "employee": employee }
            }),
            Kind::Install => serde_json::json!({
                "type": "object",
                "properties": { "code": { "type": "string", "description": "The marketplace code, WORK-XXXX-XXXX." } },
                "required": ["code"]
            }),
            Kind::Uninstall => serde_json::json!({
                "type": "object",
                "properties": { "id": { "type": "string", "description": "The install id from list_workflows." } },
                "required": ["id"]
            }),
            Kind::Create | Kind::Update => serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "The workflow's name." },
                    "definition": definition,
                    "employee": employee
                },
                "required": ["definition"]
            }),
            Kind::Delete => serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "The workflow's name." },
                    "employee": employee
                },
                "required": ["name"]
            }),
            Kind::Run => serde_json::json!({
                "type": "object",
                "properties": {
                    "workflow": workflow,
                    "inputs": { "type": "object", "description": "The workflow's input values." },
                    "employee": employee
                },
                "required": ["workflow"]
            }),
            Kind::Status | Kind::Runs => serde_json::json!({
                "type": "object",
                "properties": { "workflow": workflow, "employee": employee },
                "required": ["workflow"]
            }),
            Kind::SetEnabled => serde_json::json!({
                "type": "object",
                "properties": {
                    "workflow": workflow,
                    "enabled": { "type": "boolean", "description": "true turns it on; false turns it off." },
                    "employee": employee
                },
                "required": ["workflow", "enabled"]
            }),
        }
    }

    fn read_only(self) -> bool {
        matches!(self, Kind::List | Kind::Status | Kind::Runs)
    }

    fn labels(self, input: &serde_json::Value) -> (String, String) {
        let named = |key: &str| str_field(input, key).to_string();
        let on = input["enabled"].as_bool().unwrap_or(true);
        match self {
            Kind::List => ("checking workflows".into(), "Checked workflows".into()),
            Kind::Install => ("installing a workflow".into(), "Installed a workflow".into()),
            Kind::Uninstall => ("uninstalling a workflow".into(), "Uninstalled a workflow".into()),
            Kind::Create => (format!("creating the {} workflow", named("name")), format!("Created the {} workflow", named("name"))),
            Kind::Update => (format!("updating the {} workflow", named("name")), format!("Updated the {} workflow", named("name"))),
            Kind::Delete => (format!("deleting the {} workflow", named("name")), format!("Deleted the {} workflow", named("name"))),
            Kind::Run => (format!("starting {}", named("workflow")), format!("Started {}", named("workflow"))),
            Kind::Status | Kind::Runs => (format!("checking {}", named("workflow")), format!("Checked {}", named("workflow"))),
            Kind::SetEnabled if on => (format!("turning on {}", named("workflow")), format!("Turned on {}", named("workflow"))),
            Kind::SetEnabled => (format!("turning off {}", named("workflow")), format!("Turned off {}", named("workflow"))),
        }
    }
}

fn str_field<'a>(input: &'a serde_json::Value, key: &str) -> &'a str {
    input.get(key).and_then(|v| v.as_str()).map(str::trim).unwrap_or("")
}

/// A create or update needs the workflow's name. The model reliably puts it
/// in one of two places: the top-level `name` argument, or a `name` field in
/// the definition JSON it wrote. Both are the same fact; refusing one of them
/// with "name is required" cost a live run three identical failures.
fn workflow_name(input: &serde_json::Value) -> String {
    let name = str_field(input, "name");
    if !name.is_empty() {
        return name.to_string();
    }
    serde_json::from_str::<serde_json::Value>(str_field(input, "definition"))
        .ok()
        .and_then(|v| v.get("name").and_then(|n| n.as_str()).map(str::to_string))
        .unwrap_or_default()
}

/// Said only when the name is in neither place.
const MISSING_NAME: &str = "The workflow needs a name: pass it as `name`, or as a \"name\" \
    field inside the definition. The definition you sent has no \"name\" field either.";

fn json_result(value: serde_json::Value) -> ToolResult {
    ToolResult::ok(serde_json::to_string_pretty(&value).unwrap_or_default())
}

/// One workflow tool over the shared [`WorkflowManager`].
pub struct WorkflowTool {
    manager: Arc<dyn WorkflowManager>,
    kind: Kind,
}

/// Every workflow tool, sharing one manager.
pub fn tools(manager: Arc<dyn WorkflowManager>) -> Vec<WorkflowTool> {
    KINDS.iter().map(|&kind| WorkflowTool { manager: manager.clone(), kind }).collect()
}

impl WorkflowTool {
    /// The employee whose workflows a call manages: the calling employee
    /// (from the session key), or the one `employee` names — resolved
    /// strictly, so a typo'd name errors instead of silently self-scoping
    /// (which is how weekend workflows once landed on the assistant instead
    /// of the employee they were meant for).
    async fn employee(&self, ctx: &ToolContext, input: &serde_json::Value) -> Result<String, ToolResult> {
        match str_field(input, "employee") {
            // Canonical, helper-aware extraction (CODE_AUDITOR Rule 8).
            "" => Ok(types::keyparser::extract_agent_id(&ctx.session_key)),
            named => self.manager.resolve_agent(named).await.map_err(ToolResult::error),
        }
    }

    async fn run(&self, ctx: &ToolContext, input: serde_json::Value) -> ToolResult {
        match self.kind {
            Kind::Install => {
                return match self.manager.install(str_field(&input, "code")).await {
                    Ok(info) => json_result(serde_json::json!({ "installed": true, "workflow": info })),
                    Err(e) => ToolResult::error(format!("install failed: {e}")),
                };
            }
            Kind::Uninstall => {
                let id = str_field(&input, "id");
                return match self.manager.uninstall(id).await {
                    Ok(()) => ToolResult::ok(format!("Workflow {id} uninstalled")),
                    Err(e) => ToolResult::error(format!("uninstall failed: {e}")),
                };
            }
            _ => {}
        }

        let employee = match self.employee(ctx, &input).await {
            Ok(id) => id,
            Err(refused) => return refused,
        };
        let employee = employee.as_str();

        match self.kind {
            Kind::List => {
                let workflows = self.manager.list(employee).await;
                json_result(serde_json::json!({ "workflows": workflows, "total": workflows.len() }))
            }
            Kind::Create | Kind::Update | Kind::Delete => {
                let name = workflow_name(&input);
                if name.is_empty() {
                    return ToolResult::error(MISSING_NAME);
                }
                if employee.is_empty() {
                    return ToolResult::error(
                        "No employee in this session owns the workflow; name one with `employee`.",
                    );
                }
                let definition = str_field(&input, "definition");
                match self.kind {
                    Kind::Create => match self.manager.create(employee, &name, definition).await {
                        Ok(info) => json_result(serde_json::json!({ "created": true, "workflow": info })),
                        Err(e) => ToolResult::error(format!("create failed: {e}")),
                    },
                    Kind::Update => match self.manager.update(employee, &name, definition).await {
                        Ok(info) => json_result(serde_json::json!({ "updated": true, "workflow": info })),
                        Err(e) => ToolResult::error(format!("update failed: {e}")),
                    },
                    _ => match self.manager.delete(employee, &name).await {
                        Ok(()) => ToolResult::ok(format!("Workflow '{name}' deleted")),
                        Err(e) => ToolResult::error(format!("delete failed: {e}")),
                    },
                }
            }
            _ => self.on_workflow(employee, &input).await,
        }
    }

    /// The calls on one workflow, resolved by name or id — the employee's
    /// own workflows first, so anything list_workflows shows is reachable.
    async fn on_workflow(&self, employee: &str, input: &serde_json::Value) -> ToolResult {
        let info = match self.manager.resolve(employee, str_field(input, "workflow")).await {
            Ok(i) => i,
            Err(e) => return ToolResult::error(format!("workflow not found: {e}")),
        };
        match self.kind {
            Kind::Run => {
                let inputs = input.get("inputs").filter(|v| !v.is_null()).cloned().unwrap_or_else(|| serde_json::json!({}));
                match self.manager.run(&info.id, inputs, "agent").await {
                    Ok(run_id) => json_result(serde_json::json!({
                        "started": true,
                        "runId": run_id,
                        "workflow": info.name,
                        "message": "Started in the background. Its outcome lands in its run history; go on with other work.",
                    })),
                    Err(e) => ToolResult::error(format!("run failed: {e}")),
                }
            }
            Kind::Status => {
                let runs = self.manager.list_runs(&info.id, 1).await;
                let Some(run) = runs.first() else {
                    return ToolResult::ok(format!("No runs found for workflow {:?}", info.name));
                };
                let mut body = serde_json::to_string_pretty(run).unwrap_or_default();
                // A run in flight answers the same way every time it is
                // asked; checking again only repeats it (Underwriter,
                // 2026-09-09). Say so on the first answer.
                if matches!(run.status.as_str(), "running" | "pending") {
                    body.push_str(
                        "\n\nStill running. Checking again won't change this answer: go on with other \
                         work, or tell the owner it is running.",
                    );
                }
                let result = ToolResult::ok(body);
                // Finished run → attach the narrator's receipt so the app
                // renders a rich card (kind: run_receipt) instead of raw
                // JSON. Running/pending runs stay plain — a receipt is only
                // issued for recorded outcomes.
                if matches!(run.status.as_str(), "completed" | "failed" | "cancelled" | "interrupted" | "exited")
                    && let Some(mut receipt) = self.manager.run_receipt(&run.id).await
                {
                    if let Some(obj) = receipt.as_object_mut() {
                        obj.insert("kind".into(), serde_json::json!("run_receipt"));
                        obj.insert("workflow".into(), serde_json::json!(info.name));
                    }
                    return result.with_payload(receipt);
                }
                result
            }
            Kind::Runs => {
                let runs = self.manager.list_runs(&info.id, 10).await;
                json_result(serde_json::json!({ "runs": runs, "total": runs.len(), "workflow": info.name }))
            }
            Kind::SetEnabled => {
                let want = input["enabled"].as_bool().unwrap_or(true);
                let state = |on: bool| if on { "on" } else { "off" };
                if info.is_enabled == want {
                    return ToolResult::ok(format!("Workflow {:?} is already {}", info.name, state(want)));
                }
                match self.manager.toggle(&info.id).await {
                    Ok(enabled) => ToolResult::ok(format!("Workflow {:?} is now {}", info.name, state(enabled))),
                    Err(e) => ToolResult::error(format!("could not turn it {}: {e}", state(want))),
                }
            }
            _ => ToolResult::error(format!("{} does not act on one workflow.", self.kind.name())),
        }
    }
}

impl DynTool for WorkflowTool {
    fn name(&self) -> &str {
        self.kind.name()
    }

    fn description(&self) -> String {
        self.kind.description()
    }

    fn schema(&self) -> serde_json::Value {
        self.kind.schema()
    }

    fn search_hint(&self) -> &str {
        self.kind.search_hint()
    }

    fn read_only(&self, _input: &serde_json::Value) -> bool {
        self.kind.read_only()
    }

    /// Never alongside other calls, reads included: a run's status changes
    /// between calls, and a concurrency-safe call is held to the
    /// identical-read ceiling, which would end a turn waiting on a run.
    fn concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        false
    }

    fn activity(&self, input: &serde_json::Value) -> String {
        self.kind.labels(input).0
    }

    fn outcome(&self, input: &serde_json::Value) -> String {
        self.kind.labels(input).1
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(self.run(ctx, input))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflows::{WorkflowInfo, WorkflowRunInfo};
    use serde_json::json;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;

    type Fut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

    /// A manager that records what it was asked and holds one workflow.
    #[derive(Default)]
    struct Recorder {
        calls: Mutex<Vec<String>>,
        enabled: Mutex<bool>,
    }

    fn info(enabled: bool) -> WorkflowInfo {
        WorkflowInfo {
            id: "wf-1".into(),
            name: "Weekly Report".into(),
            version: "1".into(),
            description: String::new(),
            is_enabled: enabled,
            trigger_count: 0,
            activity_count: 1,
        }
    }

    impl Recorder {
        fn log(&self, call: String) {
            self.calls.lock().unwrap().push(call);
        }
    }

    impl WorkflowManager for Recorder {
        fn list<'a>(&'a self, agent_id: &'a str) -> Fut<'a, Vec<WorkflowInfo>> {
            self.log(format!("list {agent_id}"));
            Box::pin(async { vec![] })
        }
        fn install<'a>(&'a self, code: &'a str) -> Fut<'a, Result<WorkflowInfo, String>> {
            self.log(format!("install {code}"));
            Box::pin(async { Ok(info(true)) })
        }
        fn uninstall<'a>(&'a self, id: &'a str) -> Fut<'a, Result<(), String>> {
            self.log(format!("uninstall {id}"));
            Box::pin(async { Ok(()) })
        }
        fn resolve<'a>(&'a self, agent_id: &'a str, name_or_id: &'a str) -> Fut<'a, Result<WorkflowInfo, String>> {
            self.log(format!("resolve {agent_id} {name_or_id}"));
            let enabled = *self.enabled.lock().unwrap();
            Box::pin(async move { Ok(info(enabled)) })
        }
        fn resolve_agent<'a>(&'a self, agent_ref: &'a str) -> Fut<'a, Result<String, String>> {
            let found = agent_ref == "Content Creator";
            Box::pin(async move { if found { Ok("cc".to_string()) } else { Err("no employee named that".to_string()) } })
        }
        fn run<'a>(&'a self, id: &'a str, inputs: serde_json::Value, _trigger: &'a str) -> Fut<'a, Result<String, String>> {
            self.log(format!("run {id} {inputs}"));
            Box::pin(async { Ok("run-1".to_string()) })
        }
        fn run_status<'a>(&'a self, _run_id: &'a str) -> Fut<'a, Result<WorkflowRunInfo, String>> {
            Box::pin(async { Err("unused".to_string()) })
        }
        fn list_runs<'a>(&'a self, _workflow_id: &'a str, limit: i64) -> Fut<'a, Vec<WorkflowRunInfo>> {
            self.log(format!("runs {limit}"));
            Box::pin(async {
                vec![WorkflowRunInfo {
                    id: "run-1".into(),
                    workflow_id: "wf-1".into(),
                    status: "running".into(),
                    trigger_type: "agent".into(),
                    total_tokens_used: None,
                    error: None,
                    started_at: 0,
                    completed_at: None,
                }]
            })
        }
        fn toggle<'a>(&'a self, id: &'a str) -> Fut<'a, Result<bool, String>> {
            self.log(format!("toggle {id}"));
            let mut on = self.enabled.lock().unwrap();
            *on = !*on;
            let now = *on;
            Box::pin(async move { Ok(now) })
        }
        fn create<'a>(&'a self, agent_id: &'a str, name: &'a str, _definition: &'a str) -> Fut<'a, Result<WorkflowInfo, String>> {
            self.log(format!("create {agent_id} {name}"));
            Box::pin(async { Ok(info(true)) })
        }
        fn update<'a>(&'a self, agent_id: &'a str, name: &'a str, _definition: &'a str) -> Fut<'a, Result<WorkflowInfo, String>> {
            self.log(format!("update {agent_id} {name}"));
            Box::pin(async { Ok(info(true)) })
        }
        fn delete<'a>(&'a self, agent_id: &'a str, name: &'a str) -> Fut<'a, Result<(), String>> {
            self.log(format!("delete {agent_id} {name}"));
            Box::pin(async { Ok(()) })
        }
        fn run_inline<'a>(
            &'a self,
            _definition_json: String,
            _inputs: serde_json::Value,
            _trigger_type: &'a str,
            _trigger_detail: Option<String>,
            _agent_id: &'a str,
            _emit_source: Option<String>,
        ) -> Fut<'a, Result<String, String>> {
            Box::pin(async { Err("unused".to_string()) })
        }
        fn cancel<'a>(&'a self, run_id: &'a str) -> Fut<'a, Result<(), String>> {
            self.log(format!("cancel {run_id}"));
            Box::pin(async { Ok(()) })
        }
    }

    struct Rig {
        manager: Arc<Recorder>,
        tools: Vec<WorkflowTool>,
    }

    impl Rig {
        fn new() -> Self {
            let manager = Arc::new(Recorder::default());
            Rig { tools: tools(manager.clone()), manager }
        }
        async fn call(&self, name: &str, input: serde_json::Value) -> ToolResult {
            let ctx = ToolContext { session_key: "agent:ops:web".into(), ..ToolContext::default() };
            self.tools.iter().find(|t| t.name() == name).unwrap().execute_dyn(&ctx, input).await
        }
        fn calls(&self) -> Vec<String> {
            std::mem::take(&mut *self.manager.calls.lock().unwrap())
        }
    }

    #[test]
    fn the_name_comes_from_the_argument_or_the_definition() {
        assert_eq!(workflow_name(&json!({"name": "Top", "definition": r#"{"name":"Inner"}"#})), "Top", "the argument wins");
        assert_eq!(workflow_name(&json!({"definition": r#"{"name":"Inner","activities":[]}"#})), "Inner");
        assert_eq!(workflow_name(&json!({"definition": r#"{"activities":[]}"#})), "");
        assert_eq!(workflow_name(&json!({"definition": "not json"})), "");
    }

    /// Each tool is one manager call, scoped to the calling employee unless
    /// `employee` names another — and a name that resolves to nobody is an
    /// error, never the caller.
    #[tokio::test]
    async fn each_tool_is_one_manager_call_for_the_right_employee() {
        let rig = Rig::new();
        let r = rig.call("create_workflow", json!({"definition": r#"{"name":"Weekly Report","activities":[]}"#})).await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(rig.calls(), ["create ops Weekly Report"]);
        rig.call("update_workflow", json!({"name": "Weekly Report", "definition": "{}", "employee": "Content Creator"})).await;
        assert_eq!(rig.calls(), ["update cc Weekly Report"]);
        let typo = rig.call("delete_workflow", json!({"name": "Weekly Report", "employee": "Contnet Creator"})).await;
        assert!(typo.is_error && rig.calls().is_empty(), "{}", typo.content);
        let nameless = rig.call("create_workflow", json!({"definition": "{}"})).await;
        assert!(nameless.is_error && nameless.content.contains("`name`"), "{}", nameless.content);
        rig.call("list_workflows", json!({})).await;
        rig.call("install_workflow", json!({"code": "WORK-AAAA-BBBB"})).await;
        rig.call("uninstall_workflow", json!({"id": "wf-9"})).await;
        assert_eq!(rig.calls(), ["list ops", "install WORK-AAAA-BBBB", "uninstall wf-9"]);
        let run = rig.call("run_workflow", json!({"workflow": "Weekly Report", "inputs": {"week": "2026-39"}})).await;
        assert!(run.content.contains("run-1") && !run.content.contains("workflow_status"), "{}", run.content);
        assert_eq!(rig.calls(), ["resolve ops Weekly Report", r#"run wf-1 {"week":"2026-39"}"#]);
    }

    /// A status answer for a run in flight says checking again is useless,
    /// and never tells the model to schedule a check.
    #[tokio::test]
    async fn a_running_status_invites_no_polling() {
        let rig = Rig::new();
        let r = rig.call("workflow_status", json!({"workflow": "Weekly Report"})).await;
        assert!(r.content.contains("Still running") && r.content.contains("won't change"), "{}", r.content);
        for bait in ["schedule", "create_schedule", "in 5 minutes", "check later"] {
            assert!(!r.content.contains(bait), "{bait}: {}", r.content);
        }
    }

    /// set_workflow_enabled sets a state; it never flips one already there.
    #[tokio::test]
    async fn enabled_is_a_state_not_a_toggle() {
        let rig = Rig::new();
        let r = rig.call("set_workflow_enabled", json!({"workflow": "Weekly Report", "enabled": false})).await;
        assert!(r.content.contains("already off"), "{}", r.content);
        assert!(!rig.calls().iter().any(|c| c.starts_with("toggle")));
        let r = rig.call("set_workflow_enabled", json!({"workflow": "Weekly Report", "enabled": true})).await;
        assert!(r.content.contains("now on"), "{}", r.content);
        assert!(rig.calls().iter().any(|c| c == "toggle wf-1"));
    }

    #[test]
    fn reads_are_read_only_and_nothing_runs_alongside() {
        let rig = Rig::new();
        for t in &rig.tools {
            assert_eq!(t.read_only(&json!({})), matches!(t.name(), "list_workflows" | "workflow_status" | "list_workflow_runs"), "{}", t.name());
            assert!(!t.concurrency_safe(&json!({})), "{}", t.name());
        }
    }
}
