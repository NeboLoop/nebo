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

/// Text that says there is nothing — "None yet set for this employee.",
/// "N/A", "TBD" — every word of it one that says so; the job's own words
/// always add one of their own. Release proof of 2026-09-27
/// (correction-work-name-in-definition run 2): get_employee said "Persona:
/// none yet", and update_employee, called before it was loaded, wrote
/// "None yet set for this employee." over the employee's instructions.
fn says_nothing(text: &str) -> bool {
    const NOTHING: &[&str] = &[
        "none", "no", "not", "yet", "set", "unset", "n", "a", "na", "tbd", "todo", "placeholder", "empty", "null",
        "nil", "nothing", "blank", "pending", "for", "this", "the", "employee", "instructions", "instruction",
        "persona", "description", "is", "are", "here", "there", "any", "currently", "provided", "given", "defined",
        "specified", "available", "at", "moment", "it", "has", "have",
    ];
    let words: Vec<String> = text
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect();
    !words.is_empty() && words.iter().all(|w| NOTHING.contains(&w.as_str()))
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
            "schedule": { "type": "string", "description": "When it runs: five-field cron, minute first (\"30 8 * * *\" is 8:30 every day, \"0 9 * * 1-5\" is 9:00 on weekdays), or a phrase (\"weekdays at 9am\", \"every 2 hours\")." },
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
                        "resizable": { "type": "boolean", "description": "Default true." },
                        "fullscreen": { "type": "boolean", "description": "The page takes the whole screen (a game). Default false." },
                        "orientation": { "type": "string", "enum": ["portrait", "landscape", "any"], "description": "Default portrait." },
                        "pull_to_refresh": { "type": "boolean", "description": "true gives the page the phone's pull-down-to-reload. Default false; fullscreen apps never have it." },
                        "voice": { "type": "boolean", "description": "true puts the chat's dictate and voice buttons in the app's bar on the phone, for an app the owner directs by talking while they look at it (a design canvas). Default false; fullscreen apps never have it." }
                    }
                },
                "permissions": { "type": "array", "items": { "type": "string" }, "description": "prefix:scope entries, default [\"storage:readwrite\"]; device:motion for the gyroscope and accelerometer." }
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
            Kind::ListEmployees => "list employees and what each is doing",
            Kind::GetEmployee => "show an employee's instructions, workflows and current work",
            Kind::FindEmployees => "search the marketplace for employees to hire",
            Kind::HireEmployee => "install an employee by marketplace code",
            Kind::CreateEmployee => "make a new employee with duties",
            Kind::UpdateEmployee => "rename or change an employee or app",
            Kind::DeleteEmployee => "delete an employee, the owner approving each time",
            Kind::SetEmployeeActive => "turn an employee on or off",
            Kind::SetupEmployee => "open an employee's setup form",
            Kind::RepairEmployee => "fix an employee's broken schedules",
            Kind::ReloadEmployee => "reload or update an employee's files",
            Kind::EmployeeStats => "an employee's workflow run statistics",
        }
    }

    fn description(self) -> String {
        match self {
            Kind::ListEmployees => "Lists every employee with its description and what it is doing now: working (where, on what, how long), waiting on the owner, idle or not running. Linked employees are read from their own computers."
                .to_string(),
            Kind::GetEmployee => "Shows one employee: description, instructions, workflows and their triggers, skills, files, and its current work (the request, recent calls, latest words)."
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
                - `app` or `ui`/`ui_jsx` makes it an app with its own page; load the app-studio skill before writing one.\n\
                - To change an employee that exists, its name included, use update_employee."
                .to_string(),
            Kind::UpdateEmployee => "Changes an existing employee; only what you pass changes.\n\
                - To rename an employee or an app, pass `new_name`. A rename keeps everything: the same employee, \
                with its id, settings, schedules, memory, chats and files (an app keeps its page, source, built files, \
                data and address). Never delete and re-create an employee to rename it.\n\
                - `instructions` replaces its instructions and keeps the rest; `agent_md` replaces its whole AGENT.md.\n\
                - `automations` replaces ALL its workflows. To change some, use add_automations, remove_automations, update_automation or toggle_automation.\n\
                - `input_values` sets the values its workflows read; `inputs` changes the form that asks for them.\n\
                - The owner's own request in this chat is their yes: a change they asked for is made in the one call, whatever it adds to its job. Don't make one they didn't ask for without asking.\n\
                - Anywhere else, an edit that adds to its job drafts first: tell the owner only what is new, and on their yes call again with only the `draft_id`."
                .to_string(),
            Kind::DeleteEmployee => format!(
                "Deletes an employee: its record, workflows and schedules. Its files go to the trash for {} days.\n\
                - Only when the owner asked to delete it. The owner approves every delete on a card first.\n\
                - To rename an employee or an app, use update_employee with `new_name`, never delete and re-create.\n\
                - To stop one without losing it, turn it off with set_employee_active(active: false).",
                napp::trash::KEEP_DAYS
            ),
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
                props.insert(
                    "new_name".into(),
                    json!({ "type": "string", "description": "Rename it to this. It stays the same employee and keeps everything." }),
                );
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
            Kind::DeleteEmployee => {
                CallEffects { deletes: named(), publishes: Knowable::No, removes_employee: true, ..CallEffects::default() }
            }
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

    /// list_employees and get_employee are always loaded: the proof runs of 2026-09-26 loaded it mid-conversation in the most runs, and each mid-conversation load rewrites the cached prompt (the
    /// core budget test in the registry has the counts). The rest of the
    /// family is deferred.
    fn should_defer(&self) -> bool {
        !matches!(self.kind, Kind::ListEmployees | Kind::GetEmployee)
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
    /// said yes to — one or the other; and what it says the employee does
    /// is words about the job, never a line saying there are none.
    fn validate_input(&self, input: &serde_json::Value) -> Result<(), String> {
        if !matches!(self.kind, Kind::CreateEmployee | Kind::UpdateEmployee) {
            return Ok(());
        }
        if let (false, false) = (str_field(input, "draft_id").is_some(), str_field(input, "name").is_some()) {
            return Err(format!(
                "{} needs `name` (to draft), or `draft_id` alone (to make what the owner said yes to).",
                self.kind.name()
            ));
        }
        for key in ["instructions", "description"] {
            if let Some(text) = str_field(input, key)
                && says_nothing(text)
            {
                return Err(format!(
                    "`{key}` says there is nothing (\"{}\"); it would replace the employee's own words. \
                     Nothing was changed. Pass what the employee does, in the owner's words, or leave `{key}` out.",
                    text.trim()
                ));
            }
        }
        Ok(())
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
                Kind::ListEmployees => p.handle_list(ctx).await,
                Kind::GetEmployee => p.handle_info(&input, ctx).await,
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
    fn the_family_is_group_c_and_deferred_but_the_roster_reads() {
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
        for t in &family {
            assert_eq!(t.should_defer(), !matches!(t.name(), "list_employees" | "get_employee"), "{}", t.name());
        }
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

    /// Release proof of 2026-09-27 (correction-work-name-in-definition run
    /// 2, call #3): update_employee(name, instructions: "None yet set for
    /// this employee.") replaced the employee's instructions. A line that
    /// says there is nothing is refused before the tool runs, through the
    /// registry, and loads the tool when it wasn't; the job's own words,
    /// however short, and an empty value pass.
    #[tokio::test]
    async fn update_employee_refuses_a_line_that_says_there_is_nothing() {
        let (family, _dir) = family();
        let registry = crate::Registry::new(crate::gate::test_gate());
        for t in family {
            registry.register(Box::new(t)).await;
        }
        let ctx = ToolContext {
            declared_tools: Some(Arc::new(std::collections::HashSet::from(["get_employee".to_string()]))),
            ..ToolContext::default()
        };
        let junk = json!({"instructions": "None yet set for this employee.", "name": "cs-agent-387b239e"});
        let r = registry.execute(&ctx, "update_employee", junk).await;
        assert!(r.is_error, "{}", r.content);
        assert!(r.content.contains("`instructions` says there is nothing (\"None yet set for this employee.\")"), "{}", r.content);
        assert_eq!(r.loads.len(), 1, "the refusal loads update_employee: {}", r.content);
        for nothing in ["N/A", "TBD", "none", "Not set yet.", "No instructions provided."] {
            assert!(says_nothing(nothing), "{nothing}");
            let t = registry.get("create_employee").await.unwrap();
            assert!(t.validate_input(&json!({"name": "x", "description": nothing})).is_err(), "{nothing}");
        }
        for words in ["Answers the phone.", "You are the billing desk.", "Set up new client files", "No refunds without the owner's yes."] {
            assert!(!says_nothing(words), "{words}");
        }
        let t = registry.get("update_employee").await.unwrap();
        assert!(t.validate_input(&json!({"name": "x", "instructions": ""})).is_ok());
        assert!(t.validate_input(&json!({"name": "x", "instructions": "Answers the phone."})).is_ok());
    }

    /// Release proof of 2026-09-27 (plugin-many-skills-fan-out run 2, calls
    /// #33 and #61): create_employee reported "Created employee
    /// 'billing-desk-40458bec'", and update_employee and reload_employee
    /// answered "No employee named 'billing-desk-40458bec'" until the display
    /// name was used. Every employee tool takes the short name, the id or
    /// the display name in any case, by the one resolver.
    #[tokio::test]
    async fn every_employee_tool_takes_the_short_name_the_id_or_the_name() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(db::Store::new(&dir.path().join("e.db").to_string_lossy()).unwrap());
        let id = "42d998e0-f1fc-4969-bedc-ea4cfabe00fa";
        store
            .create_agent(id, None, "Billing Desk 40458bec", "Handles billing.", "---\nname: billing-desk-40458bec\n---\nYou are the billing desk.", "", None, None)
            .unwrap();
        let loader = Arc::new(napp::AgentLoader::new(dir.path().join("a"), dir.path().join("b")));
        let live = Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
        let family = tools(PersonaTool::new(store.clone(), live, loader));
        let ctx = ToolContext::default();
        for label in ["billing-desk-40458bec", "Billing Desk 40458bec", "billing desk 40458BEC", id] {
            for name in ["get_employee", "update_employee", "reload_employee", "employee_stats", "setup_employee", "repair_employee"] {
                let mut input = json!({"name": label});
                if name == "update_employee" {
                    input["description"] = json!("Handles billing and payments.");
                }
                let r = tool(&family, name).execute_dyn(&ctx, input).await;
                assert!(!r.content.contains("No employee named"), "{name}({label}): {}", r.content);
            }
        }
        assert_eq!(store.get_agent(id).unwrap().unwrap().description, "Handles billing and payments.");
        let r = tool(&family, "update_employee").execute_dyn(&ctx, json!({"name": "billing-desk", "description": "x y"})).await;
        assert!(r.content.contains("No employee named 'billing-desk'"), "a near miss names no one: {}", r.content);
        let r = tool(&family, "delete_employee").execute_dyn(&ctx, json!({"name": "billing-desk-40458bec"})).await;
        assert!(!r.is_error, "{}", r.content);
        assert!(store.get_agent(id).unwrap().is_none());
    }

    /// An app employee, its folder named for it under the loader's user
    /// folder, with a page, source, settings, a workflow and a schedule.
    fn app_employee(dir: &std::path::Path, store: &db::Store, id: &str, name: &str) -> std::path::PathBuf {
        let md = format!("---\nname: {name}\n---\nYou are {name}.");
        store.create_agent(id, Some("user"), name, "A game.", &md, "", None, None).unwrap();
        let folder = dir.join("b").join(name);
        std::fs::create_dir_all(folder.join("ui").join("assets")).unwrap();
        std::fs::create_dir_all(folder.join("src")).unwrap();
        std::fs::write(folder.join("AGENT.md"), &md).unwrap();
        std::fs::write(folder.join("ui").join("index.html"), "<div id=root></div>").unwrap();
        std::fs::write(folder.join("ui").join("assets").join("index.js"), "built").unwrap();
        std::fs::write(folder.join("src").join("App.tsx"), "export default function App() {}").unwrap();
        store.set_agent_napp_path(id, &folder.to_string_lossy()).unwrap();
        let ui = folder.join("ui");
        store.set_agent_app_fields(id, true, Some(&ui.to_string_lossy()), None, Some("{\"fullscreen\":true}")).unwrap();
        store.update_agent_input_values(id, "{\"level\":\"hard\"}").unwrap();
        store
            .upsert_agent_workflow(id, "nightly", "schedule", "0 3 * * *", Some("Nightly"), None, None, None, None, false)
            .unwrap();
        store
            .create_cron_job(&format!("agent-{id}-nightly"), "0 3 * * *", "", "agent", None, None, None, true, Some(id), None, None)
            .unwrap();
        folder
    }

    fn family_in(dir: &std::path::Path, store: &Arc<db::Store>) -> Vec<EmployeeTool> {
        let loader = Arc::new(napp::AgentLoader::new(dir.join("a"), dir.join("b")));
        let live = Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
        tools(PersonaTool::new(store.clone(), live, loader))
    }

    /// 2026-10-01: told there was no rename, an employee deleted the app
    /// "Tweet" and made "Flip-Flap", and the app's source went with it. A
    /// rename is the same employee under a new name: its id, settings,
    /// workflows, schedules, page, source and built files all stay, and its
    /// folder moves with the name, the row repointed with it.
    #[tokio::test]
    async fn a_rename_keeps_everything_and_moves_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(db::Store::new(&dir.path().join("e.db").to_string_lossy()).unwrap());
        let id = "ab788706-2a52-4b38-8a5b-2d2335ef8475";
        let old = app_employee(dir.path(), &store, id, "Tweet");
        let family = family_in(dir.path(), &store);

        let r = tool(&family, "update_employee")
            .execute_dyn(&ToolContext::default(), json!({"name": "Tweet", "new_name": "Flip-Flap"}))
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.contains("the same employee"), "{}", r.content);

        let row = store.get_agent(id).unwrap().expect("the same row, by the same id");
        assert_eq!(row.name, "Flip-Flap");
        assert_eq!(row.input_values, "{\"level\":\"hard\"}", "its settings stay");
        assert_eq!(row.app_window_config.as_deref(), Some("{\"fullscreen\":true}"));
        let moved = dir.path().join("b").join("Flip-Flap");
        assert_eq!(row.napp_path.as_deref(), Some(moved.to_str().unwrap()), "the row names the moved folder");
        assert_eq!(row.app_ui_path.as_deref(), Some(moved.join("ui").to_str().unwrap()), "its page is served from there");
        assert!(!old.exists(), "no folder left under the old name");
        assert_eq!(std::fs::read_to_string(moved.join("src").join("App.tsx")).unwrap(), "export default function App() {}");
        assert_eq!(std::fs::read_to_string(moved.join("ui").join("assets").join("index.js")).unwrap(), "built");
        assert_eq!(store.list_agent_workflows(id).unwrap().len(), 1, "its workflows stay");
        let schedules = store.list_cron_jobs(100, 0).unwrap();
        assert!(schedules.iter().any(|j| j.name == format!("agent-{id}-nightly")), "its schedules stay");

        // A later edit lands in the moved folder.
        let r = tool(&family, "update_employee")
            .execute_dyn(&ToolContext::default(), json!({"name": "Flip-Flap", "instructions": "You are a flapping game."}))
            .await;
        assert!(!r.is_error, "{}", r.content);
        assert!(std::fs::read_to_string(moved.join("AGENT.md")).unwrap().contains("a flapping game"));
    }

    /// Names are unique per bot: a rename onto another employee's name, in
    /// any spelling, is refused and changes nothing, files included.
    #[tokio::test]
    async fn a_rename_onto_a_taken_name_is_refused_and_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(db::Store::new(&dir.path().join("e.db").to_string_lossy()).unwrap());
        let folder = app_employee(dir.path(), &store, "tweet-id", "Tweet");
        store.create_agent("other", None, "Flip Flap", "", "", "{}", None, None).unwrap();
        let family = family_in(dir.path(), &store);

        let r = tool(&family, "update_employee")
            .execute_dyn(
                &ToolContext::default(),
                json!({"name": "Tweet", "new_name": "flip-flap", "description": "A different game."}),
            )
            .await;
        assert!(r.is_error, "{}", r.content);
        assert!(r.content.contains("already have an employee named \"Flip Flap\""), "{}", r.content);
        let row = store.get_agent("tweet-id").unwrap().unwrap();
        assert_eq!((row.name.as_str(), row.description.as_str()), ("Tweet", "A game."), "nothing was changed");
        assert_eq!(row.napp_path.as_deref(), Some(folder.to_str().unwrap()));
        assert!(folder.join("src").join("App.tsx").exists());
    }

    /// A delete never wipes an app's files: they are moved whole into the
    /// trash, where they can be put back.
    #[tokio::test]
    async fn a_deleted_apps_files_are_kept_in_the_trash() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(db::Store::new(&dir.path().join("e.db").to_string_lossy()).unwrap());
        let folder = app_employee(dir.path(), &store, "tweet-id", "Tweet");
        let family = family_in(dir.path(), &store);

        let r = tool(&family, "delete_employee").execute_dyn(&ToolContext::default(), json!({"name": "Tweet"})).await;
        assert!(!r.is_error, "{}", r.content);
        assert!(store.get_agent("tweet-id").unwrap().is_none());
        assert!(!folder.exists());
        let trash = dir.path().join("trash");
        let slot = std::fs::read_dir(&trash).unwrap().next().unwrap().unwrap().path();
        let kept = slot.join("Tweet");
        assert!(r.content.contains(&kept.display().to_string()), "the result says where: {}", r.content);
        assert_eq!(std::fs::read_to_string(kept.join("src").join("App.tsx")).unwrap(), "export default function App() {}");
        assert_eq!(std::fs::read_to_string(kept.join("ui").join("index.html")).unwrap(), "<div id=root></div>");
    }

    /// The model reads how to rename where it looks: the search hint and
    /// update_employee say rename keeps everything; delete_employee says
    /// never to delete to rename, and that the owner approves each one.
    #[test]
    fn rename_is_offered_where_the_model_looks() {
        let (family, _dir) = family();
        let update = tool(&family, "update_employee");
        assert!(update.search_hint().contains("rename"));
        assert!(update.description().contains("A rename keeps everything"), "{}", update.description());
        assert!(update.description().contains("Never delete and re-create"), "{}", update.description());
        let delete = tool(&family, "delete_employee").description();
        assert!(delete.contains("never delete and re-create") && delete.contains("approves every delete"), "{delete}");
        assert!(tool(&family, "delete_employee").effects(&json!({"name": "Tweet"})).removes_employee);
        assert!(!update.effects(&json!({"name": "Tweet", "new_name": "Flip-Flap"})).removes_employee);
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
