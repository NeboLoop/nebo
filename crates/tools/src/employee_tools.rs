//! The employee tools: the roster on this computer, the marketplace's hires,
//! and making, changing and removing employees. One purpose per tool, each a
//! thin interface over the [`PersonaTool`] handlers, which do the work.

use std::sync::Arc;

use crate::agent_tool::PersonaTool;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};
use types::permissions::{CallEffects, Knowable};

/// One tool of the employee family.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    ListEmployees,
    GetEmployee,
    FindEmployees,
    HireEmployee,
    CreateEmployee,
    UpdateEmployee,
    DeleteEmployee,
    SetEmployeeActive,
    SetupEmployee,
    RepairEmployee,
    ReloadEmployee,
    EmployeeStats,
}

const KINDS: &[Kind] = &[
    Kind::ListEmployees,
    Kind::GetEmployee,
    Kind::FindEmployees,
    Kind::HireEmployee,
    Kind::CreateEmployee,
    Kind::UpdateEmployee,
    Kind::DeleteEmployee,
    Kind::SetEmployeeActive,
    Kind::SetupEmployee,
    Kind::RepairEmployee,
    Kind::ReloadEmployee,
    Kind::EmployeeStats,
];

/// The marketplace's departments, the one list `find_employees` offers.
const DEPARTMENTS: &str = "accounting, sales, customer-support, marketing, direct-response, operations, people-hr, \
     legal, it, analytics, product-engineering, executive, corporate";

fn str_field<'a>(input: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    input.get(key).and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty())
}

/// The name property every tool that acts on one employee declares.
fn name_param() -> serde_json::Value {
    serde_json::json!({ "type": "string", "description": "The employee's name or id, as list_employees shows it." })
}

/// The draft the owner said yes to: `create_employee` and
/// `update_employee` send it alone to make exactly what was drafted.
fn draft_param() -> serde_json::Value {
    serde_json::json!({ "type": "string", "description": "The draft the owner said yes to, from the first call's result. Send it alone." })
}

/// One recurring or triggered duty, the item `automations` and
/// `add_automations` take.
fn automation_item() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "name": { "type": "string", "description": "Workflow name, e.g. weekday-page-check." },
            "description": { "type": "string", "description": "What the duty is, in a line." },
            "schedule": { "type": "string", "description": "When it runs: a cron (\"0 9 * * 1-5\") or a phrase (\"weekdays at 9am\", \"every 2 hours\")." },
            "interval": { "type": "string", "description": "Instead of a schedule: run every interval (\"15m\", \"1h\")." },
            "window": { "type": "string", "description": "With interval: the hours it runs in, e.g. \"08:00-18:00\"." },
            "sources": { "type": "array", "items": { "type": "string" }, "minItems": 1, "description": "Instead of a schedule: the events that start it, e.g. \"email.received\"." },
            "steps": { "type": "array", "items": { "type": "string" }, "description": "Concrete ordered steps, run in order in one run: what to do, with which tool or data, producing what." },
            "activities": {
                "type": "array",
                "description": "Instead of steps, for a duty in stages: each stage runs on its own and sees the earlier stages' outputs.",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "intent": { "type": "string", "description": "What this stage accomplishes, in a line." },
                        "steps": { "type": "array", "items": { "type": "string" } },
                        "skills": { "type": "array", "items": { "type": "string" }, "description": "Skills this stage may use." }
                    },
                    "required": ["id", "intent", "steps"]
                }
            },
            "emit": { "type": "string", "description": "An event to emit when it finishes, e.g. briefing.ready." }
        },
        "required": ["name"]
    })
}

/// The properties `create_employee` and `update_employee` share: what the
/// employee does and, for an app, its page.
fn job_properties() -> serde_json::Map<String, serde_json::Value> {
    let props = serde_json::json!({
        "description": { "type": "string", "description": "What the employee does, in a sentence or two." },
        "agent_md": { "type": "string", "description": "The whole AGENT.md (frontmatter and instructions). Rarely needed: description and instructions cover most employees." },
        "automations": { "type": "array", "items": automation_item(), "description": "The employee's recurring or triggered duties; each becomes its own workflow, run as the employee." },
        "app": {
            "type": "object",
            "description": "Make the employee an app with its own page. {} is fine.",
            "properties": {
                "window": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "width": { "type": "integer", "description": "Default 1024." },
                        "height": { "type": "integer", "description": "Default 768." },
                        "resizable": { "type": "boolean", "description": "Default true." }
                    }
                },
                "permissions": { "type": "array", "items": { "type": "string" }, "description": "prefix:scope entries, default [\"storage:readwrite\"]." }
            }
        },
        "ui": { "type": "object", "additionalProperties": { "type": "string" }, "description": "The app's page files: relative path → content, e.g. {\"index.html\": \"...\"}." },
        "ui_jsx": { "type": "string", "description": "Instead of ui: one .jsx/.tsx file that `export default`s a React component, compiled into the app's page." }
    });
    match props {
        serde_json::Value::Object(map) => map,
        _ => unreachable!("a JSON object literal"),
    }
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::ListEmployees => "list_employees",
            Kind::GetEmployee => "get_employee",
            Kind::FindEmployees => "find_employees",
            Kind::HireEmployee => "hire_employee",
            Kind::CreateEmployee => "create_employee",
            Kind::UpdateEmployee => "update_employee",
            Kind::DeleteEmployee => "delete_employee",
            Kind::SetEmployeeActive => "set_employee_active",
            Kind::SetupEmployee => "setup_employee",
            Kind::RepairEmployee => "repair_employee",
            Kind::ReloadEmployee => "reload_employee",
            Kind::EmployeeStats => "employee_stats",
        }
    }

    fn search_hint(self) -> &'static str {
        match self {
            Kind::ListEmployees => "list the employees and apps here",
            Kind::GetEmployee => "show an employee's instructions and workflows",
            Kind::FindEmployees => "search the marketplace for employees to hire",
            Kind::HireEmployee => "install an employee by marketplace code",
            Kind::CreateEmployee => "make a new employee with duties",
            Kind::UpdateEmployee => "change an employee's instructions or workflows",
            Kind::DeleteEmployee => "permanently remove an employee",
            Kind::SetEmployeeActive => "turn an employee on or off",
            Kind::SetupEmployee => "open an employee's setup form",
            Kind::RepairEmployee => "fix an employee's broken schedules",
            Kind::ReloadEmployee => "reload or update an employee's files",
            Kind::EmployeeStats => "an employee's workflow run statistics",
        }
    }

    fn description(self) -> String {
        match self {
            Kind::ListEmployees => "Lists the employees on this computer — hired from the marketplace, made here, and apps — each with its description."
                .to_string(),
            Kind::GetEmployee => "Shows one employee: its description, instructions, workflows and what starts them, skills, and where its files are."
                .to_string(),
            Kind::FindEmployees => format!(
                "Searches the marketplace for employees to hire, and the tools they use.\n\
                - Pass every role the owner named as a list in one call (`query: [\"bookkeeper\", \"social media manager\"]`); never search roles one at a time. Leave `query` out to browse the catalog.\n\
                - Results mark who is already hired and put NeboAI's own employees first ([NeboAI]); prefer them.\n\
                - One hire card offers the best match for every query; the owner hires with one tap. Never paste install codes into chat.\n\
                - Never say the marketplace has nothing until this tool says so.\n\
                - The employee is the hire; a tool is what it uses. {}",
                crate::plugin_tool::TOOL_INSTALL_DOOR
            ),
            Kind::HireEmployee => "Installs from the marketplace by install code (AGNT-XXXX-XXXX; skill, plugin and collection codes work too), with everything it depends on.\n\
                - Codes are issued by the marketplace, never made from a name. To hire someone, find them with find_employees; its card hires them.\n\
                - An employee already in list_employees needs no install."
                .to_string(),
            Kind::CreateEmployee => "Makes a new employee: its name, what it does, and its duties. It takes two calls.\n\
                - The first call drafts it and returns one plain line of what it will be able to do; nothing is created yet. Say that line to the owner and ask them to confirm, unless their latest message already told you to create it now.\n\
                - On their yes, or at once when they already told you to create it, call again with only the `draft_id`: that creates exactly the drafted job. If they want it different, draft again.\n\
                - Every recurring duty goes in `automations`: each becomes the employee's own workflow, run as it. Never make separate schedules for it.\n\
                - Steps must be concrete — which tools, files and destinations, what to check, what to produce — because the workflow runs unattended on these words alone.\n\
                - `app` or `ui`/`ui_jsx` makes it an app with its own page; load the build-an-app skill before writing one.\n\
                - To change an employee that exists, use update_employee."
                .to_string(),
            Kind::UpdateEmployee => "Changes an existing employee; only what you pass changes.\n\
                - `instructions` replaces its instructions and keeps the rest; `agent_md` replaces its whole AGENT.md.\n\
                - `automations` replaces ALL its workflows. To change some, use add_automations, remove_automations, update_automation or toggle_automation.\n\
                - `input_values` sets the values its workflows read; `inputs` changes the form that asks for them.\n\
                - `new_name` renames it.\n\
                - The owner's own request in this chat is their yes: a change they asked for is made in the one call, whatever it adds to its job. Don't make one they didn't ask for without asking.\n\
                - Anywhere else, an edit that adds to its job drafts first: tell the owner only what is new, and on their yes call again with only the `draft_id`."
                .to_string(),
            Kind::DeleteEmployee => "Permanently deletes an employee: its record, workflows, schedules and the files it was made with.\n\
                - It can't be undone; only when the owner asked for it."
                .to_string(),
            Kind::SetEmployeeActive => "Turns an employee on or off: on loads it and starts its schedules; off unloads it.".to_string(),
            Kind::SetupEmployee => "Opens an employee's setup form in the app, where the owner fills in its inputs and schedules.".to_string(),
            Kind::RepairEmployee => "Fixes employees' schedules: invalid schedule expressions, schedules left behind and triggers out of step with the workflows.\n\
                - Leave `name` out to repair every employee."
                .to_string(),
            Kind::ReloadEmployee => "Reads an employee's AGENT.md and agent.json from disk into the app again, after its files were edited.\n\
                - check_update: see whether the marketplace has a newer version (hired employees).\n\
                - apply_update: download and apply the newest version."
                .to_string(),
            Kind::EmployeeStats => "Shows an employee's workflow runs: totals, completed and failed, tokens used and recent errors.".to_string(),
        }
    }

    fn schema(self) -> serde_json::Value {
        use serde_json::json;
        match self {
            Kind::ListEmployees => json!({ "type": "object", "properties": {} }),
            Kind::GetEmployee | Kind::DeleteEmployee | Kind::SetupEmployee | Kind::EmployeeStats => json!({
                "type": "object",
                "properties": { "name": name_param() },
                "required": ["name"]
            }),
            Kind::FindEmployees => json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": ["string", "array"],
                        "items": { "type": "string" },
                        "description": "The roles or tools to find: one, or a list of up to 10 answered in one call."
                    },
                    "department": { "type": "string", "description": format!("Keep to one department: {DEPARTMENTS}.") },
                    "limit": { "type": "integer", "description": "Results per query, default 20." },
                    "offset": { "type": "integer", "description": "Page offset when browsing without a query." }
                }
            }),
            Kind::HireEmployee => json!({
                "type": "object",
                "properties": { "code": { "type": "string", "description": "The marketplace install code, e.g. AGNT-ABCD-1234." } },
                "required": ["code"]
            }),
            Kind::CreateEmployee => {
                let mut props = job_properties();
                props.insert("name".into(), json!({ "type": "string", "description": "The employee's name, e.g. \"Office Manager\"." }));
                props.insert(
                    "requires".into(),
                    json!({
                        "type": "object",
                        "description": "What it needs from the start. plugins: installed plugin slugs it uses; tools: tool names it has loaded from its first turn; interfaces: capabilities it binds (e.g. \"ledger\", \"mail\").",
                        "properties": {
                            "plugins": { "type": "array", "items": { "type": "string" } },
                            "tools": { "type": "array", "items": { "type": "string" } },
                            "interfaces": { "type": "array", "items": { "type": "string" } }
                        }
                    }),
                );
                props.insert(
                    "agent_json".into(),
                    json!({ "type": ["string", "object"], "description": "A raw agent.json (workflows, triggers, skills). Rarely needed: automations covers it." }),
                );
                props.insert("draft_id".into(), draft_param());
                json!({ "type": "object", "properties": props })
            }
            Kind::UpdateEmployee => {
                let mut props = job_properties();
                props.insert("name".into(), name_param());
                props.insert("new_name".into(), json!({ "type": "string", "description": "Rename it to this." }));
                props.insert(
                    "instructions".into(),
                    json!({ "type": "string", "description": "Its new instructions: the body of AGENT.md, replaced; its settings and workflows stay." }),
                );
                props.insert(
                    "add_automations".into(),
                    json!({ "type": "array", "items": automation_item(), "description": "Workflows to add, keeping the others." }),
                );
                props.insert(
                    "remove_automations".into(),
                    json!({ "type": "array", "items": { "type": "string" }, "description": "Workflows to remove, by name (their schedules go too)." }),
                );
                props.insert(
                    "update_automation".into(),
                    json!({
                        "type": "object",
                        "description": "Change one workflow, by name; only the fields you pass change.",
                        "properties": {
                            "name": { "type": "string" },
                            "description": { "type": "string" },
                            "steps": { "type": "array", "items": { "type": "string" } },
                            "schedule": { "type": "string" },
                            "interval": { "type": "string" },
                            "window": { "type": "string" },
                            "sources": { "type": "array", "items": { "type": "string" } },
                            "emit": { "type": "string" }
                        },
                        "required": ["name"]
                    }),
                );
                props.insert("toggle_automation".into(), json!({ "type": "string", "description": "Turn one workflow on or off, by name." }));
                props.insert(
                    "input_values".into(),
                    json!({ "type": "object", "description": "Values for its inputs, key → value; every workflow run reads them." }),
                );
                props.insert(
                    "inputs".into(),
                    json!({
                        "type": "array",
                        "description": "The form that asks for its input values (shown on its Settings tab), replaced.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "key": { "type": "string" },
                                "label": { "type": "string" },
                                "type": { "type": "string", "enum": ["text", "textarea", "number", "select", "checkbox", "radio", "path", "file"] },
                                "description": { "type": "string" },
                                "required": { "type": "boolean" },
                                "default": {},
                                "placeholder": { "type": "string" },
                                "options": { "type": "array", "items": { "type": "object", "properties": { "value": { "type": "string" }, "label": { "type": "string" } } } }
                            },
                            "required": ["key", "label"]
                        }
                    }),
                );
                props.insert("draft_id".into(), draft_param());
                json!({ "type": "object", "properties": props })
            }
            Kind::SetEmployeeActive => json!({
                "type": "object",
                "properties": {
                    "name": name_param(),
                    "active": { "type": "boolean", "description": "true turns it on, false turns it off." }
                },
                "required": ["name", "active"]
            }),
            Kind::RepairEmployee => json!({
                "type": "object",
                "properties": { "name": { "type": "string", "description": "The employee to repair; leave out to repair every one." } }
            }),
            Kind::ReloadEmployee => json!({
                "type": "object",
                "properties": {
                    "name": name_param(),
                    "check_update": { "type": "boolean", "description": "See whether the marketplace has a newer version." },
                    "apply_update": { "type": "boolean", "description": "Download and apply the newest version." }
                },
                "required": ["name"]
            }),
        }
    }

    fn read_only(self) -> bool {
        matches!(self, Kind::ListEmployees | Kind::GetEmployee | Kind::FindEmployees | Kind::EmployeeStats)
    }

    fn effects(self, input: &serde_json::Value) -> CallEffects {
        let named = || str_field(input, "name").map(str::to_string).into_iter().collect::<Vec<_>>();
        match self {
            Kind::DeleteEmployee => CallEffects { deletes: named(), publishes: Knowable::No, ..CallEffects::default() },
            Kind::UpdateEmployee => CallEffects { overwrites: named(), publishes: Knowable::No, ..CallEffects::default() },
            Kind::ReloadEmployee if input.get("apply_update").and_then(|v| v.as_bool()) == Some(true) => {
                CallEffects { overwrites: named(), publishes: Knowable::No, ..CallEffects::default() }
            }
            // Everything else stays on this computer.
            _ => CallEffects::none(),
        }
    }

    fn labels(self, input: &serde_json::Value) -> (String, String) {
        let who = str_field(input, "name").unwrap_or("an employee");
        let pair = |a: String, b: String| (a, b);
        match self {
            Kind::ListEmployees => pair("listing employees".into(), "Listed employees".into()),
            Kind::GetEmployee => pair(format!("reading about {who}"), format!("Read about {who}")),
            Kind::FindEmployees => pair("searching the marketplace".into(), "Searched the marketplace".into()),
            Kind::HireEmployee => pair("hiring from the marketplace".into(), "Hired from the marketplace".into()),
            Kind::CreateEmployee => pair(format!("making {who}"), format!("Made {who}")),
            Kind::UpdateEmployee => pair(format!("updating {who}"), format!("Updated {who}")),
            Kind::DeleteEmployee => pair(format!("deleting {who}"), format!("Deleted {who}")),
            Kind::SetEmployeeActive => match input.get("active").and_then(|v| v.as_bool()) {
                Some(false) => pair(format!("turning off {who}"), format!("Turned off {who}")),
                _ => pair(format!("turning on {who}"), format!("Turned on {who}")),
            },
            Kind::SetupEmployee => pair(format!("opening {who}'s setup"), format!("Opened {who}'s setup")),
            Kind::RepairEmployee => match str_field(input, "name") {
                Some(n) => pair(format!("repairing {n}'s schedules"), format!("Repaired {n}'s schedules")),
                None => pair("repairing schedules".into(), "Repaired schedules".into()),
            },
            Kind::ReloadEmployee => pair(format!("reloading {who}"), format!("Reloaded {who}")),
            Kind::EmployeeStats => pair(format!("reading {who}'s runs"), format!("Read {who}'s runs")),
        }
    }
}

/// One employee tool (see [`Kind`] for the family).
pub struct EmployeeTool {
    persona: Arc<PersonaTool>,
    kind: Kind,
}

/// Every tool of the employee family, sharing the one set of handlers.
pub fn tools(persona: PersonaTool) -> Vec<EmployeeTool> {
    let persona = Arc::new(persona);
    KINDS.iter().map(|&kind| EmployeeTool { persona: persona.clone(), kind }).collect()
}

impl DynTool for EmployeeTool {
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

    /// A marketplace search can park on the owner's hire card, so it runs
    /// alone; the other reads run beside other reads.
    fn concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        self.kind.read_only() && self.kind != Kind::FindEmployees
    }

    fn effects(&self, input: &serde_json::Value) -> CallEffects {
        self.kind.effects(input)
    }

    /// A create or update names its employee, or sends the draft the owner
    /// said yes to — one or the other.
    fn validate_input(&self, input: &serde_json::Value) -> Result<(), String> {
        if !matches!(self.kind, Kind::CreateEmployee | Kind::UpdateEmployee) {
            return Ok(());
        }
        match (str_field(input, "draft_id").is_some(), str_field(input, "name").is_some()) {
            (false, false) => Err(format!(
                "{} needs `name` (to draft), or `draft_id` alone (to make what the owner said yes to).",
                self.kind.name()
            )),
            _ => Ok(()),
        }
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
        Box::pin(async move {
            let p = &self.persona;
            match self.kind {
                Kind::ListEmployees => p.handle_list().await,
                Kind::GetEmployee => p.handle_info(&input).await,
                Kind::FindEmployees => p.handle_discover(&input, ctx).await,
                Kind::HireEmployee => p.handle_install(ctx, &input).await,
                Kind::CreateEmployee => p.create_with_consent(&input, ctx).await,
                Kind::UpdateEmployee => p.update_with_consent(&input, ctx).await,
                Kind::DeleteEmployee => p.handle_delete(&input).await,
                Kind::SetEmployeeActive => match input.get("active").and_then(|v| v.as_bool()) {
                    Some(false) => p.handle_deactivate(&input).await,
                    _ => p.handle_activate(&input).await,
                },
                Kind::SetupEmployee => p.handle_setup(&input).await,
                Kind::RepairEmployee => p.handle_repair(&input).await,
                Kind::ReloadEmployee => p.handle_reload(&input).await,
                Kind::EmployeeStats => p.handle_stats(&input).await,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn family() -> (Vec<EmployeeTool>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(db::Store::new(&dir.path().join("e.db").to_string_lossy()).unwrap());
        let loader = Arc::new(napp::AgentLoader::new(dir.path().join("a"), dir.path().join("b")));
        let registry = Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
        (tools(PersonaTool::new(store, registry, loader)), dir)
    }

    fn tool<'a>(family: &'a [EmployeeTool], name: &str) -> &'a EmployeeTool {
        family.iter().find(|t| t.name() == name).unwrap()
    }

    /// The family is the design's group C, every one deferred.
    #[test]
    fn the_family_is_group_c_and_deferred() {
        let (family, _dir) = family();
        let names: Vec<&str> = family.iter().map(|t| t.name()).collect();
        assert_eq!(
            names,
            [
                "list_employees", "get_employee", "find_employees", "hire_employee", "create_employee",
                "update_employee", "delete_employee", "set_employee_active", "setup_employee",
                "repair_employee", "reload_employee", "employee_stats"
            ]
        );
        assert!(family.iter().all(|t| t.should_defer()));
    }

    /// The model reads these, and the owner hears it back: employees, never
    /// "agent", and no call shape of the retired tool.
    #[test]
    fn the_texts_say_employee_and_name_only_current_tools() {
        let (family, _dir) = family();
        for t in &family {
            let text = format!("{}\n{}", t.description(), t.schema());
            assert!(!text.contains("agent("), "{}: {text}", t.name());
            assert!(!text.contains("resource"), "{}: {text}", t.name());
            assert!(!text.to_lowercase().contains(" agent "), "{}: {text}", t.name());
            let (doing, done) = (t.activity(&json!({"name": "Front Desk"})), t.outcome(&json!({"name": "Front Desk"})));
            assert!(!doing.contains("agent") && !done.contains("agent"), "{doing} / {done}");
        }
    }

    /// Hiring guidance lives where the model reads it: the marketplace
    /// search. On 2026-09-14 it lived on a tool the model never saw, and the
    /// model kept saying the marketplace was empty.
    #[test]
    fn find_employees_carries_the_hiring_guidance() {
        let (family, _dir) = family();
        let d = tool(&family, "find_employees").description();
        assert!(d.contains("never search roles one at a time"), "{d}");
        assert!(d.contains("[NeboAI]"), "{d}");
        assert!(d.contains("The employee is the hire"), "{d}");
        assert!(d.contains(crate::plugin_tool::TOOL_INSTALL_DOOR), "{d}");
    }

    #[test]
    fn reads_are_read_only_and_a_search_that_can_park_runs_alone() {
        let (family, _dir) = family();
        for name in ["list_employees", "get_employee", "employee_stats"] {
            assert!(tool(&family, name).read_only(&json!({})), "{name}");
            assert!(tool(&family, name).concurrency_safe(&json!({})), "{name}");
        }
        assert!(tool(&family, "find_employees").read_only(&json!({})));
        assert!(!tool(&family, "find_employees").concurrency_safe(&json!({})));
        for name in ["hire_employee", "create_employee", "update_employee", "delete_employee", "set_employee_active"] {
            assert!(!tool(&family, name).read_only(&json!({})), "{name}");
        }
    }

    #[test]
    fn a_delete_says_what_it_deletes() {
        let (family, _dir) = family();
        let e = tool(&family, "delete_employee").effects(&json!({"name": "Front Desk"}));
        assert_eq!(e.deletes, ["Front Desk"]);
        assert_eq!(e.publishes, Knowable::No);
        let e = tool(&family, "update_employee").effects(&json!({"name": "Front Desk"}));
        assert_eq!(e.overwrites, ["Front Desk"]);
        assert!(tool(&family, "create_employee").effects(&json!({"name": "x"})).deletes.is_empty());
    }

    /// A create or update drafts from a name or makes a draft the owner
    /// said yes to; a call with neither is refused before it runs.
    #[test]
    fn a_create_or_update_names_the_employee_or_its_draft() {
        let (family, _dir) = family();
        for name in ["create_employee", "update_employee"] {
            let t = tool(&family, name);
            assert!(t.validate_input(&json!({"description": "x"})).is_err(), "{name}");
            assert!(t.validate_input(&json!({"name": "Front Desk"})).is_ok(), "{name}");
            assert!(t.validate_input(&json!({"draft_id": "d-1"})).is_ok(), "{name}");
            assert!(t.schema()["properties"]["draft_id"].is_object(), "{name}");
        }
    }

    /// correction-agent-update-description run 3: the model's first call
    /// sent the schema's own help text as the new description, and it was
    /// saved. The registry refuses it by name before the tool runs.
    #[tokio::test]
    async fn update_employee_refuses_its_own_help_text_as_the_description() {
        let (family, _dir) = family();
        let registry = crate::Registry::new(crate::gate::test_gate());
        let update = family.into_iter().find(|t| t.name() == "update_employee").unwrap();
        registry.register(Box::new(update)).await;
        let ctx = ToolContext::default();
        let echo = json!({"description": "What the employee does, in a sentence or two.", "name": "front-desk-63ed55f1"});
        let r = registry.execute(&ctx, "update_employee", echo).await;
        assert!(r.is_error, "{}", r.content);
        assert!(r.content.contains("The parameter `description` is its own help text"), "{}", r.content);
        assert!(r.content.contains("Pass the owner's actual words for `description`"), "{}", r.content);
        // The owner's words reach the tool (which finds no such employee here).
        let real = json!({"description": "Answers inbound calls for NeboAI and takes messages for the team.", "name": "front-desk-63ed55f1"});
        let r = registry.execute(&ctx, "update_employee", real).await;
        assert!(!r.content.contains("help text"), "{}", r.content);
        assert!(r.content.contains("No employee named 'front-desk-63ed55f1'"), "{}", r.content);
    }

    /// Making an employee goes through the one consent step: with the
    /// permission system not yet bound, nothing is created.
    #[tokio::test]
    async fn create_goes_through_the_consent_step() {
        let (family, dir) = family();
        let r = tool(&family, "create_employee")
            .execute_dyn(&ToolContext::default(), json!({"name": "front-desk", "description": "Answers calls"}))
            .await;
        assert!(r.is_error && r.content.contains("permissions aren't ready"), "{}", r.content);
        assert!(!dir.path().join("b").join("front-desk").exists());
    }

    /// Turning an employee off goes to the handler that unloads it.
    #[tokio::test]
    async fn active_false_unloads() {
        let (family, _dir) = family();
        let ctx = ToolContext::default();
        let r = tool(&family, "set_employee_active")
            .execute_dyn(&ctx, json!({"name": "front-desk", "active": false}))
            .await;
        assert!(r.content.contains("is not active"), "{}", r.content);
    }
}
