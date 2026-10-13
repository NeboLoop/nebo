//! Wiring checks for a workflow definition, run when a definition is saved,
//! installed or published, and again (errors only) when a workflow parses
//! for a run.
//!
//! A definition is the workflow JSON shape both a standalone workflow and an
//! agent.json binding serialize to: `activities` (id, type, intent, steps,
//! params) and `connections` (from, to, label).
//!
//! Severity rule: an **error** is something certainly broken, a step that
//! cannot do what it says on any run, and it blocks a save or a publish. A
//! **warning** is something that runs but is likely wrong, and it is shown
//! to the author without blocking.
//!
//! | Rule | Severity | Why |
//! |---|---|---|
//! | condition expression does not parse, unknown mode, invalid regex | error | the step fails on every run |
//! | bare name in a condition without exactly one parent step | error | there is no single output to read it from |
//! | `nodes.<id>` naming no step | error | the data never exists |
//! | `nodes.<id>` in a condition, loop source or params that is not an upstream step | error | a deterministic step reads it before it can exist, or races it |
//! | `${nodes.…}`, `${inputs.…}`, `${item…}` in params | error | never substituted; the data syntax is `{{nodes.…}}` |
//! | step not reachable from the trigger | error | it never runs |
//! | `nodes.<id>` in an AI step's text that is not an upstream step | warning | the step may run before it, or never see it |
//! | loop without `params.concurrency` | warning | every item starts at once against the same systems |
//! | AI step fed an `operation` step's whole list (its parent, or `nodes.<op>` in its text) | warning | thousands of records reach one step as a summary it pages through; a loop hands them over one at a time |
//!
//! The audit ([`audit_workflow`]) runs these checks and fixes what it can
//! determine, so a workflow runs under the engine as written:
//! - each AI step's `tools`, from what its past runs called, the tools its
//!   words name, and what the employee declares (its plugins, the step's
//!   skills, connected MCP servers), and, for an open-ended step none of
//!   those resolve, the tools picked for it ([`Signals::picked`]);
//! - a step whose words name a step upstream of it that is not its parent
//!   gets that step's data by reference, `{{nodes.<id>}}`;
//! - a condition that does not parse and has one obvious reading
//!   (`and` for `&&`, `=` for `==`), `${…}` placeholders and
//!   `nodes.__trigger__` reads are rewritten. Anything else is reported.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::condition::{self, Reference};

const TRIGGER: &str = "__trigger__";

/// Step types that run without a model; every other step is an AI step.
const DETERMINISTIC: [&str; 8] = ["condition", "loop", "wait", "http", "command", "operation", "decide", "expert"];
const EMIT: &str = "__emit__";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// One finding about a definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Issue {
    pub severity: Severity,
    /// The workflow (binding name or workflow id).
    pub workflow: String,
    /// The step it is about, when it is about one.
    #[serde(rename = "step")]
    pub activity: Option<String>,
    pub message: String,
}

impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let level = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        match &self.activity {
            Some(a) => write!(f, "{level}: workflow '{}', step '{a}': {}", self.workflow, self.message),
            None => write!(f, "{level}: workflow '{}': {}", self.workflow, self.message),
        }
    }
}

/// The errors among `issues`, one per line, or None when there are none.
pub fn error_text(issues: &[Issue]) -> Option<String> {
    let errors: Vec<String> = issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .map(|i| i.to_string())
        .collect();
    (!errors.is_empty()).then(|| errors.join("\n"))
}

/// The errors in `after` that `before` did not already have, one per line.
/// An edit is refused for what it breaks, not for a defect it did not touch
/// (an installed employee's broken step must not block renaming it).
pub fn new_error_text(before: &[Issue], after: &[Issue]) -> Option<String> {
    let fresh: Vec<Issue> = after
        .iter()
        .filter(|i| i.severity == Severity::Error && !before.contains(i))
        .cloned()
        .collect();
    error_text(&fresh)
}

/// Check every standard workflow an agent.json carries. Call trees are
/// phone-line configuration, not engine workflows, and are skipped.
pub fn check_agent(config: &crate::agent::AgentConfig) -> Vec<Issue> {
    let mut names: Vec<&String> = config.workflows.keys().collect();
    names.sort();
    let mut out = Vec::new();
    for name in names {
        let binding = &config.workflows[name];
        if binding.is_call_tree() || binding.activities.is_empty() {
            continue;
        }
        match serde_json::to_value(binding) {
            Ok(v) => out.extend(check_workflow(name, &v)),
            Err(e) => out.push(Issue {
                severity: Severity::Error,
                workflow: name.clone(),
                activity: None,
                message: format!("could not be read: {e}"),
            }),
        }
    }
    out
}

struct Node<'a> {
    id: &'a str,
    kind: &'a str,
    raw: &'a Value,
}

/// Check one workflow definition.
pub fn check_workflow(name: &str, def: &Value) -> Vec<Issue> {
    let mut issues = Vec::new();
    let nodes: Vec<Node> = def
        .get("activities")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|raw| Node {
                    id: raw.get("id").and_then(Value::as_str).unwrap_or(""),
                    kind: raw.get("type").and_then(Value::as_str).unwrap_or(""),
                    raw,
                })
                .collect()
        })
        .unwrap_or_default();
    let edges: Vec<(&str, &str)> = def
        .get("connections")
        .and_then(Value::as_array)
        .map(|c| {
            c.iter()
                .map(|e| {
                    (
                        e.get("from").and_then(Value::as_str).unwrap_or(""),
                        e.get("to").and_then(Value::as_str).unwrap_or(""),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let ids: HashSet<&str> = nodes.iter().map(|n| n.id).collect();

    // Upstream steps: with connections, everything with a path into the
    // step; without, every step before it in array order (the sequential
    // path runs them in order).
    let mut parents: HashMap<&str, Vec<&str>> = HashMap::new();
    for &(from, to) in &edges {
        if from != TRIGGER && to != EMIT {
            let list = parents.entry(to).or_default();
            if !list.contains(&from) {
                list.push(from);
            }
        }
    }
    let graph = !edges.is_empty();
    let upstream = |id: &str| -> HashSet<&str> {
        if !graph {
            return nodes.iter().take_while(|n| n.id != id).map(|n| n.id).collect();
        }
        let mut seen = HashSet::new();
        let mut queue: Vec<&str> = parents.get(id).cloned().unwrap_or_default();
        while let Some(p) = queue.pop() {
            if p == id || !seen.insert(p) {
                continue;
            }
            queue.extend(parents.get(p).cloned().unwrap_or_default());
        }
        seen
    };

    let mut push = |severity: Severity, activity: &str, message: String| {
        issues.push(Issue {
            severity,
            workflow: name.to_string(),
            activity: Some(activity.to_string()),
            message,
        });
    };

    for node in &nodes {
        let ups = upstream(node.id);
        let params = node.raw.get("params");
        let param = |key: &str| {
            params
                .and_then(|p| p.get(key))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
        };

        // A `nodes.<id>` reference a deterministic step reads.
        let node_ref = |push: &mut dyn FnMut(Severity, &str, String), target: &str, how: &str| {
            if !ids.contains(target) {
                push(
                    Severity::Error,
                    node.id,
                    missing_step(how, target),
                );
            } else if !ups.contains(target) {
                push(
                    Severity::Error,
                    node.id,
                    format!(
                        "{how} reads nodes.{target}, but '{target}' is not upstream of this step; \
                         connect '{target}' into it (directly or through earlier steps)"
                    ),
                );
            }
        };

        if node.kind == "condition" {
            let expression = param("expression");
            let mode = match param("mode") {
                "" => "expression",
                m => m,
            };
            match mode {
                "expression" => match condition::parse(expression) {
                    Err(e) => push(
                        Severity::Error,
                        node.id,
                        format!("condition `{expression}` does not parse: {e}"),
                    ),
                    Ok(expr) => {
                        let refs = condition::references(&expr);
                        let step_parents: Vec<&str> =
                            parents.get(node.id).cloned().unwrap_or_default();
                        if let Some(Reference::Parent(bare)) =
                            refs.iter().find(|r| matches!(r, Reference::Parent(_)))
                        {
                            if step_parents.len() != 1 {
                                push(
                                    Severity::Error,
                                    node.id,
                                    format!(
                                        "condition reads '{bare}' without a prefix, which means a field of the one \
                                         step feeding this condition, but it has {} incoming steps; write \
                                         nodes.<step>.{bare} or inputs.{bare}",
                                        step_parents.len()
                                    ),
                                );
                            }
                        }
                        for r in refs {
                            if let Some(target) = r.node_id() {
                                node_ref(&mut push, target, "condition");
                            }
                        }
                        for word in bare_right_operands(&expr) {
                            push(
                                Severity::Warning,
                                node.id,
                                format!(
                                    "condition compares against {word} unquoted, which reads a field named \
                                     '{word}' from the step feeding it; if you meant the text, write '{word}'"
                                ),
                            );
                        }
                    }
                },
                "regex" => {
                    if let Err(e) = regex::Regex::new(expression) {
                        push(Severity::Error, node.id, format!("condition regex does not compile: {e}"));
                    }
                }
                "exists" | "contains" => {
                    let path = if mode == "contains" {
                        expression.split_once(" contains ").map(|(l, _)| l.trim()).unwrap_or("")
                    } else {
                        expression
                    };
                    if let Some(target) = path.strip_prefix("nodes.").map(|r| r.split('.').next().unwrap_or("")) {
                        node_ref(&mut push, target, "condition");
                    }
                }
                other => push(
                    Severity::Error,
                    node.id,
                    format!("condition mode '{other}' is not one of expression, contains, exists, regex"),
                ),
            }
        }

        if node.kind == "loop" {
            if let Some(target) = param("source").strip_prefix("nodes.").map(|r| r.split('.').next().unwrap_or("")) {
                node_ref(&mut push, target, "loop source");
            }
            if params.and_then(|p| p.get("concurrency")).is_none() {
                push(
                    Severity::Warning,
                    node.id,
                    "loop sets no params.concurrency, so every item starts at once; set 1 for \
                     order-dependent writes, or a small number for reads against one system"
                        .into(),
                );
            }
        }

        // Every text param: placeholder syntax and the data it reads.
        let mut texts = Vec::new();
        if let Some(p) = params {
            collect_strings(p, &mut texts);
        }
        for text in &texts {
            for bad in ["${nodes.", "${inputs.", "${item"] {
                if let Some(start) = text.find(bad) {
                    let end = text[start..].find('}').map(|i| start + i + 1).unwrap_or(text.len());
                    let shown = &text[start..end];
                    let inner = shown.trim_start_matches("${").trim_end_matches('}');
                    let fixed = format!("{{{{{inner}}}}}");
                    push(
                        Severity::Error,
                        node.id,
                        format!("{shown} is never filled in; write {fixed} (the one data placeholder syntax)"),
                    );
                }
            }
            for path in placeholders(text) {
                if let Some(target) = path.strip_prefix("nodes.").map(|r| r.split('.').next().unwrap_or("")) {
                    node_ref(&mut push, target, "params");
                }
            }
        }

        // AI step text: a node it names must exist; one that is not
        // upstream is a warning (it may not have run yet).
        let mut prose: Vec<&str> = vec![node.raw.get("intent").and_then(Value::as_str).unwrap_or("")];
        if let Some(steps) = node.raw.get("steps").and_then(Value::as_array) {
            prose.extend(steps.iter().filter_map(Value::as_str));
        }
        let mut named: Vec<&str> = Vec::new();
        for text in prose {
            for target in prose_node_refs(text) {
                if !named.contains(&target) {
                    named.push(target);
                }
            }
        }
        // An AI step fed an operation step's whole list: every page of
        // records reaches one step, which can only page through them.
        if !DETERMINISTIC.contains(&node.kind) {
            let fed: Vec<&str> = if graph {
                parents.get(node.id).cloned().unwrap_or_default()
            } else {
                nodes.iter().take_while(|n| n.id != node.id).last().map(|n| n.id).into_iter().collect()
            };
            let lists: Vec<&str> = nodes
                .iter()
                .filter(|n| n.kind == "operation" && n.raw.pointer("/params/rows").and_then(Value::as_str).is_none_or(|r| r.trim().is_empty()))
                .map(|n| n.id)
                .filter(|op| fed.contains(op) || named.contains(op))
                .collect();
            for op in lists {
                push(
                    Severity::Warning,
                    node.id,
                    format!(
                        "this AI step is handed every record '{op}' reads (all its pages), which can be thousands: \
                         it gets a summary and pages through them. Loop over nodes.{op}.records and hand the step \
                         one record (or a batchSize) at a time"
                    ),
                );
            }
        }
        for target in named {
            if !ids.contains(target) {
                push(
                    Severity::Error,
                    node.id,
                    missing_step("its text", target),
                );
            } else if target != node.id && !ups.contains(target) {
                push(
                    Severity::Warning,
                    node.id,
                    format!(
                        "its text reads nodes.{target}, but '{target}' is not upstream of this step, \
                         so its output may not exist yet; connect '{target}' into it"
                    ),
                );
            }
        }
    }

    // Reachability: the walk starts at the trigger's edges (or the first
    // step when there are none) and follows every edge.
    if graph {
        let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
        for &(from, to) in &edges {
            adjacency.entry(from).or_default().push(to);
        }
        let mut queue: VecDeque<&str> = match adjacency.get(TRIGGER) {
            Some(entries) => entries.iter().copied().collect(),
            None => nodes.first().map(|n| n.id).into_iter().collect(),
        };
        let mut reached: HashSet<&str> = HashSet::new();
        while let Some(n) = queue.pop_front() {
            if reached.insert(n) {
                queue.extend(adjacency.get(n).cloned().unwrap_or_default());
            }
        }
        for node in &nodes {
            if !reached.contains(node.id) {
                issues.push(Issue {
                    severity: Severity::Error,
                    workflow: name.to_string(),
                    activity: Some(node.id.to_string()),
                    message: "no path from the trigger reaches this step, so it never runs; \
                              connect it from the step that should come before it"
                        .into(),
                });
            }
        }
    }

    issues
}

/// The error for a `nodes.<id>` reference naming no step.
fn missing_step(how: &str, target: &str) -> String {
    if target == TRIGGER {
        format!(
            "{how} reads nodes.{TRIGGER}, which is never set; the trigger's data is \
             inputs.<field> (in params, {{{{inputs.<field>}}}})"
        )
    } else {
        format!("{how} reads nodes.{target}, but there is no step '{target}'")
    }
}

/// Bare names on the right of a comparison: almost always text the author
/// forgot to quote (`kind == won`).
fn bare_right_operands(expr: &condition::Expr) -> Vec<&str> {
    use condition::{Expr, Operand};
    let mut out = Vec::new();
    fn walk<'e>(e: &'e Expr, out: &mut Vec<&'e str>) {
        match e {
            Expr::Or(a, b) | Expr::And(a, b) => {
                walk(a, out);
                walk(b, out);
            }
            Expr::Not(a) => walk(a, out),
            Expr::Compare(_, _, Operand::Ref(Reference::Parent(w))) => out.push(w),
            _ => {}
        }
    }
    walk(expr, &mut out);
    out
}

fn collect_strings<'v>(v: &'v Value, out: &mut Vec<&'v str>) {
    match v {
        Value::String(s) => out.push(s),
        Value::Array(a) => a.iter().for_each(|x| collect_strings(x, out)),
        Value::Object(m) => m.values().for_each(|x| collect_strings(x, out)),
        _ => {}
    }
}

/// The paths inside `{{ ... }}` placeholders.
fn placeholders(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        out.push(after[..end].trim());
        rest = &after[end + 2..];
    }
    out
}

/// Node ids named as `nodes.<id>` in free text. The id stops at the first
/// character that cannot be in one; a trailing '-' (prose punctuation) is
/// dropped.
fn prose_node_refs(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(pos) = text[from..].find("nodes.") {
        let at = from + pos;
        let boundary = at == 0 || {
            let prev = text[..at].chars().next_back().unwrap_or(' ');
            !(prev.is_alphanumeric() || prev == '_' || prev == '.' || prev == '$')
        };
        let start = at + "nodes.".len();
        let len = text[start..]
            .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
            .unwrap_or(text.len() - start);
        let id = text[start..start + len].trim_end_matches('-');
        if boundary && !id.is_empty() {
            out.push(id);
        }
        from = start;
    }
    out
}

// ── The audit ──────────────────────────────────────────────────────────

/// What the audit reads to declare each AI step's tools. Every tool it adds
/// is in `catalog`.
pub struct Signals<'a> {
    /// The tools a step may be given: the ones this Nebo has now, never a
    /// scheduling tool or the runtime set every step already has.
    pub catalog: &'a BTreeSet<String>,
    /// Each step (activity id) that ran before, with the tools its runs
    /// called. Any name; only catalog names are added.
    pub history: &'a HashMap<String, BTreeSet<String>>,
    /// The tools each skill brings to a step that declares it.
    pub skill_tools: &'a HashMap<String, Vec<String>>,
    /// The employee's required plugins, as their tools (`plugin__<slug>`).
    pub plugins: &'a [String],
    /// Open-ended steps the shortlist and the model resolved: step → its
    /// tools (empty: it needs none).
    pub picked: &'a HashMap<String, Vec<String>>,
}

/// Where a step's tools came from, strongest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// The tools its past runs called.
    History,
    /// A tool its words name.
    Named,
    /// What the employee declares: its plugins, the step's skills, `cmds`
    /// and `mcps`, a connected MCP server the step names.
    Declared,
    /// The embedding shortlist and the model's pick from it.
    Model,
}

/// A tool the audit added to a step's declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AddedTool {
    pub tool: String,
    pub source: Source,
}

/// One AI step's tools as the audit settled them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StepTools {
    pub step: String,
    pub added: Vec<AddedTool>,
    /// The strongest signal that told what the step needs; `None`: none did
    /// (the step is in [`Audit::unresolved`]).
    #[serde(rename = "resolvedBy")]
    pub resolved_by: Option<Source>,
}

/// A change the audit made to a step other than its tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Fix {
    pub step: String,
    pub message: String,
}

/// What [`audit_workflow`] found and did.
#[derive(Debug, Clone)]
pub struct Audit {
    /// The definition with every fix applied.
    pub definition: Value,
    /// Every AI step the audit declared tools for (a `tools: []` judging
    /// step is not one).
    pub steps: Vec<StepTools>,
    pub fixes: Vec<Fix>,
    /// Open-ended AI steps no signal resolved: the shortlist and the model
    /// are for these.
    pub unresolved: Vec<String>,
    /// What is still wrong: the checks of the fixed definition.
    pub issues: Vec<Issue>,
}

impl Audit {
    /// Whether the audit changed the definition.
    pub fn changed(&self) -> bool {
        !self.fixes.is_empty() || self.steps.iter().any(|s| !s.added.is_empty())
    }
}

/// Audit one workflow definition: fix what can be determined, declare each
/// AI step's tools from `signals`, and check what is left.
pub fn audit_workflow(name: &str, def: &Value, signals: &Signals<'_>) -> Audit {
    let mut definition = def.clone();
    let mut fixes = Vec::new();
    fix_placeholders(&mut definition, &mut fixes);
    fix_conditions(&mut definition, &mut fixes);
    fix_references(&mut definition, &mut fixes);
    let (steps, unresolved) = declare_tools(&mut definition, signals);
    let issues = check_workflow(name, &definition);
    Audit { definition, steps, fixes, unresolved, issues }
}

/// The hash of what a workflow runs: its activities and connections. Two
/// definitions that run the same have the same hash.
pub fn definition_hash(def: &Value) -> String {
    let shape = serde_json::json!({
        "activities": def.get("activities").cloned().unwrap_or(Value::Null),
        "connections": def.get("connections").cloned().unwrap_or(Value::Null),
    });
    sha256(&shape.to_string())
}

/// The key a step's picked tools are kept under: the step as written, its
/// `tools` aside (what the pick fills in), and the tool catalog it was
/// picked from. Either changing is a new pick.
pub fn step_key(activity: &Value, catalog_hash: &str) -> String {
    let mut step = activity.clone();
    if let Some(o) = step.as_object_mut() {
        o.remove("tools");
    }
    sha256(&format!("{step}\n{catalog_hash}"))
}

/// The hex sha256 of `text`.
pub fn sha256(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// Whether an activity is an AI step: a step a model runs, which uses
/// tools.
pub fn is_ai_step(activity: &Value) -> bool {
    !DETERMINISTIC.contains(&activity.get("type").and_then(Value::as_str).unwrap_or(""))
}

/// A step's own words: its intent and its steps, one per line.
pub fn step_text(activity: &Value) -> String {
    let mut out = activity.get("intent").and_then(Value::as_str).unwrap_or("").to_string();
    for step in activity.get("steps").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        out.push('\n');
        out.push_str(step);
    }
    out
}

/// Whether a `tools` entry names tool `n`: its exact name, a dotted prefix
/// (`"odoo"` covers every `odoo.*` tool) or a `prefix*` family
/// (`"mcp__github__*"` covers that server's tools).
pub fn names_tool(entry: &str, n: &str) -> bool {
    n == entry
        || n.strip_prefix(entry).is_some_and(|r| r.starts_with('.'))
        || entry.strip_suffix('*').is_some_and(|p| !p.is_empty() && n.starts_with(p))
}

/// The tools `text` names, among those `known` accepts, in order: a name
/// written as a call (`name(`), a name no prose word could be (one with an
/// underscore), and an operation written dotted, with or without a seat's
/// `<department>.<role>.` prefix (`accounting.invoice-sender.ledger.invoice.search`),
/// as the tool it is called through (`ledger_invoice_search`).
pub fn named_tools(text: &str, known: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |name: String| {
        if !out.contains(&name) {
            out.push(name);
        }
    };
    for (at, word) in name_words(text) {
        let called = text[at + word.len()..].starts_with('(');
        if (called || word.contains('_')) && known(word) {
            push(word.to_string());
        }
    }
    for dotted in dotted_words(text) {
        let segments: Vec<&str> = dotted.split('.').filter(|s| !s.is_empty()).collect();
        for k in (2..=segments.len()).rev() {
            let tool = segments[segments.len() - k..].join("_").to_lowercase().replace('-', "_");
            if known(&tool) {
                push(tool);
                break;
            }
        }
    }
    out
}

/// Each run of tool-name characters (lowercase letters, digits, `_`) in
/// `text`, with where it starts.
pub fn name_words(text: &str) -> Vec<(usize, &str)> {
    let is_name = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_';
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in text.char_indices() {
        match (is_name(c), start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                out.push((s, &text[s..i]));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((s, &text[s..]));
    }
    out
}

/// Each dotted word in `text` (`a.b.c`, letters, digits, `_`, `-` between
/// the dots), its trailing punctuation dropped.
fn dotted_words(text: &str) -> Vec<&str> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
        .map(|w| w.trim_matches(|c: char| matches!(c, '.' | '-')))
        .filter(|w| w.contains('.') && !w.starts_with("nodes.") && !w.starts_with("inputs."))
        .collect()
}

/// Whether `text` names `word` as a whole word, any case: not inside a
/// longer name.
fn mentions(text: &str, word: &str) -> bool {
    if word.is_empty() {
        return false;
    }
    let lower = text.to_lowercase();
    let word = word.to_lowercase();
    let inside = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-');
    let mut from = 0;
    while let Some(pos) = lower[from..].find(&word) {
        let at = from + pos;
        let end = at + word.len();
        if !inside(lower[..at].chars().next_back()) && !inside(lower[end..].chars().next()) {
            return true;
        }
        from = end;
    }
    false
}

/// Rewrite every string in `v` that `f` changes; whether any changed.
fn rewrite_strings(v: &mut Value, f: &dyn Fn(&str) -> Option<String>) -> bool {
    match v {
        Value::String(s) => match f(s) {
            Some(new) => {
                *s = new;
                true
            }
            None => false,
        },
        // Every string is rewritten: no short circuit.
        Value::Array(a) => a.iter_mut().map(|x| rewrite_strings(x, f)).filter(|changed| *changed).count() > 0,
        Value::Object(m) => m.values_mut().map(|x| rewrite_strings(x, f)).filter(|changed| *changed).count() > 0,
        _ => false,
    }
}

/// `${nodes.…}`, `${inputs.…}` and `${item…}` in params are never filled
/// in: they become `{{…}}`. `nodes.__trigger__.<field>` is never set: the
/// trigger's data is `inputs.<field>`.
fn fix_placeholders(def: &mut Value, fixes: &mut Vec<Fix>) {
    let rewrite = |text: &str| -> Option<String> {
        let mut out = text.to_string();
        for bad in ["${nodes.", "${inputs.", "${item"] {
            while let Some(start) = out.find(bad) {
                let Some(len) = out[start..].find('}') else { break };
                let inner = out[start + 2..start + len].to_string();
                out.replace_range(start..start + len + 1, &format!("{{{{{inner}}}}}"));
            }
        }
        out = out.replace(&format!("nodes.{TRIGGER}."), "inputs.");
        (out != text).then_some(out)
    };
    for activity in def.get_mut("activities").and_then(Value::as_array_mut).into_iter().flatten() {
        let id = activity.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        if let Some(params) = activity.get_mut("params")
            && rewrite_strings(params, &rewrite)
        {
            fixes.push(Fix {
                step: id,
                message: "its data placeholders now use {{…}} and read the trigger's data as inputs.<field>".into(),
            });
        }
    }
}

/// A condition that does not parse but has one obvious reading is
/// rewritten to it: `and`/`or`/`not` for `&&`/`||`/`!`, a single `=` or
/// `===` for `==`, and a `{{…}}` or `${…}` placeholder for the path inside
/// it. The rewrite stands only when it parses.
fn fix_conditions(def: &mut Value, fixes: &mut Vec<Fix>) {
    for activity in def.get_mut("activities").and_then(Value::as_array_mut).into_iter().flatten() {
        if activity.get("type").and_then(Value::as_str) != Some("condition") {
            continue;
        }
        let mode = activity.pointer("/params/mode").and_then(Value::as_str).unwrap_or("");
        if !mode.is_empty() && mode != "expression" {
            continue;
        }
        let Some(expression) = activity.pointer("/params/expression").and_then(Value::as_str).map(str::to_string) else {
            continue;
        };
        if let Some(repaired) = repair_condition(&expression) {
            let id = activity.get("id").and_then(Value::as_str).unwrap_or("").to_string();
            activity["params"]["expression"] = Value::String(repaired.clone());
            fixes.push(Fix { step: id, message: format!("condition `{expression}` now reads `{repaired}`") });
        }
    }
}

/// The one reading of a condition that does not parse, when it has one.
fn repair_condition(expression: &str) -> Option<String> {
    if expression.trim().is_empty() || condition::parse(expression).is_ok() {
        return None;
    }
    let chars: Vec<char> = expression.chars().collect();
    let mut out = String::new();
    let mut quote: Option<char> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote {
            out.push(c);
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                out.push(c);
                i += 1;
            }
            '=' => {
                let run = chars[i..].iter().take_while(|c| **c == '=').count();
                if matches!(out.chars().last(), Some('!' | '<' | '>')) {
                    out.push_str(&"=".repeat(run));
                } else {
                    out.push_str("==");
                }
                i += run;
            }
            '{' | '$' if chars.get(i + 1) == Some(&'{') => {
                // `{{path}}` or `${path}`: the path itself.
                let close = if c == '{' { "}}" } else { "}" };
                let rest: String = chars[i + 2..].iter().collect();
                let end = rest.find(close)?;
                out.push_str(rest[..end].trim());
                i += 2 + rest[..end].chars().count() + close.len();
            }
            c if c.is_alphabetic() => {
                let word: String = chars[i..]
                    .iter()
                    .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '-'))
                    .collect();
                i += word.chars().count();
                match word.to_lowercase().as_str() {
                    "and" => out.push_str("&&"),
                    "or" => out.push_str("||"),
                    "not" => out.push('!'),
                    _ => out.push_str(&word),
                }
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    condition::parse(&out).is_ok().then_some(out)
}

/// An AI step whose words name a step upstream of it that is not its
/// parent reads that step's data only by reference: `{{nodes.<id>}}` is
/// added to its task. A step is named as `nodes.<id>`, or by an id no
/// prose word could be (one with `-` or `_`). Steps inside a loop body
/// read only their own item and iteration, so they are left to the report.
fn fix_references(def: &mut Value, fixes: &mut Vec<Fix>) {
    let graph = Graph::of(def);
    let mut additions: Vec<(usize, Vec<String>)> = Vec::new();
    for (index, (id, kind, text)) in graph.nodes.iter().enumerate() {
        if DETERMINISTIC.contains(&kind.as_str()) || graph.in_loop_body(id) {
            continue;
        }
        let parents = graph.result_parents(id);
        let referenced: HashSet<&str> = placeholders(text)
            .into_iter()
            .filter_map(|p| p.strip_prefix("nodes."))
            .map(|r| r.split('.').next().unwrap_or(""))
            .collect();
        let prose = prose_node_refs(text);
        let mut wanted = Vec::new();
        for up in graph.upstream(id) {
            if up == *id
                || parents.contains(&up)
                || referenced.contains(up.as_str())
                || matches!(graph.kind_of(&up), "condition" | "wait")
            {
                continue;
            }
            let named = prose.contains(&up.as_str()) || ((up.contains('-') || up.contains('_')) && mentions(text, &up));
            if named {
                wanted.push(up);
            }
        }
        if !wanted.is_empty() {
            wanted.sort_by_key(|w| graph.index_of(w));
            additions.push((index, wanted));
        }
    }
    let Some(activities) = def.get_mut("activities").and_then(Value::as_array_mut) else { return };
    for (index, wanted) in additions {
        let activity = &mut activities[index];
        let id = activity.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        let mut intent = activity.get("intent").and_then(Value::as_str).unwrap_or("").to_string();
        for up in &wanted {
            intent.push_str(&format!("\n\nStep {up}'s output: {{{{nodes.{up}}}}}"));
            fixes.push(Fix {
                step: id.clone(),
                message: format!("it reads step '{up}', which is not its parent: added {{{{nodes.{up}}}}}"),
            });
        }
        activity["intent"] = Value::String(intent);
    }
}

/// Declare each AI step's tools from the signals. A step with `tools: []`
/// is a judging step and stays one.
fn declare_tools(def: &mut Value, signals: &Signals<'_>) -> (Vec<StepTools>, Vec<String>) {
    let mut steps = Vec::new();
    let mut unresolved = Vec::new();
    let in_catalog = |n: &str| signals.catalog.contains(n);
    let servers: BTreeSet<&str> = signals
        .catalog
        .iter()
        .filter_map(|n| n.strip_prefix("mcp__").and_then(|r| r.split_once("__")).map(|(s, _)| s))
        .collect();
    for activity in def.get_mut("activities").and_then(Value::as_array_mut).into_iter().flatten() {
        if !is_ai_step(activity) {
            continue;
        }
        let declared = activity.get("tools").and_then(Value::as_array).map(|a| {
            a.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<String>>()
        });
        if declared.as_ref().is_some_and(Vec::is_empty) {
            continue;
        }
        let entries = declared.unwrap_or_default();
        let id = activity.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        let text = step_text(activity);
        let list = |key: &str| -> Vec<String> {
            activity.get(key).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
        };
        let (skills, cmds, mcps) = (list("skills"), list("cmds"), list("mcps"));

        let ran = signals.history.get(&id);
        let history: Vec<String> = ran.into_iter().flatten().filter(|t| in_catalog(t)).cloned().collect();
        let named = named_tools(&text, &in_catalog);
        let mut declared_tools: Vec<String> = signals
            .plugins
            .iter()
            .filter(|p| in_catalog(p))
            .filter(|p| p.strip_prefix(PLUGIN_PREFIX).is_some_and(|slug| mentions(&text, slug)))
            .cloned()
            .collect();
        for server in &servers {
            if mentions(&text, server) || mentions(&text, &server.replace('_', " ")) || mentions(&text, &server.replace('_', "-")) {
                declared_tools.push(format!("mcp__{server}__*"));
            }
        }
        let brings = skills.iter().any(|s| signals.skill_tools.get(s).is_some_and(|t| !t.is_empty()));
        let picked = signals.picked.get(&id);

        let resolved_by = if ran.is_some() {
            Some(Source::History)
        } else if !named.is_empty() {
            Some(Source::Named)
        } else if !declared_tools.is_empty() || brings || !cmds.is_empty() || !mcps.is_empty() {
            Some(Source::Declared)
        } else if picked.is_some() {
            Some(Source::Model)
        } else {
            None
        };
        let model: Vec<String> = match resolved_by {
            Some(Source::Model) => picked.into_iter().flatten().filter(|t| in_catalog(t)).cloned().collect(),
            _ => Vec::new(),
        };
        let mut added: Vec<AddedTool> = Vec::new();
        for (tools, source) in [
            (history, Source::History),
            (named, Source::Named),
            (declared_tools, Source::Declared),
            (model, Source::Model),
        ] {
            for tool in tools {
                let covered = entries.iter().any(|e| names_tool(e, &tool))
                    || added.iter().any(|a| a.tool == tool || names_tool(&a.tool, &tool));
                if !covered {
                    added.push(AddedTool { tool, source });
                }
            }
        }
        // A server's family covers the tools of it named one by one.
        let families: Vec<String> = added.iter().filter(|a| a.tool.ends_with('*')).map(|a| a.tool.clone()).collect();
        added.retain(|a| a.tool.ends_with('*') || !families.iter().any(|f| names_tool(f, &a.tool)));
        if resolved_by.is_none() {
            unresolved.push(id.clone());
        }
        if !added.is_empty() {
            let mut tools = entries.clone();
            tools.extend(added.iter().map(|a| a.tool.clone()));
            activity["tools"] = Value::from(tools);
        }
        steps.push(StepTools { step: id, added, resolved_by });
    }
    (steps, unresolved)
}

/// A plugin's tool name prefix (`plugin__<slug>`).
const PLUGIN_PREFIX: &str = "plugin__";

/// The wiring of a definition as the fixes read it.
struct Graph {
    /// (id, type, words) in array order.
    nodes: Vec<(String, String, String)>,
    edges: Vec<(String, String, Option<String>)>,
}

impl Graph {
    fn of(def: &Value) -> Self {
        let nodes = def
            .get("activities")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|a| {
                (
                    a.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                    a.get("type").and_then(Value::as_str).unwrap_or("").to_string(),
                    step_text(a),
                )
            })
            .collect();
        let edges = def
            .get("connections")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|e| {
                (
                    e.get("from").and_then(Value::as_str).unwrap_or("").to_string(),
                    e.get("to").and_then(Value::as_str).unwrap_or("").to_string(),
                    e.get("label").and_then(Value::as_str).map(str::to_string),
                )
            })
            .collect();
        Self { nodes, edges }
    }

    fn index_of(&self, id: &str) -> usize {
        self.nodes.iter().position(|(n, ..)| n == id).unwrap_or(usize::MAX)
    }

    fn kind_of(&self, id: &str) -> &str {
        self.nodes.iter().find(|(n, ..)| n == id).map(|(_, k, _)| k.as_str()).unwrap_or("")
    }

    /// Direct parents; on the sequential path (no connections), the step
    /// before.
    fn parents(&self, id: &str) -> Vec<String> {
        if self.edges.is_empty() {
            let i = self.index_of(id);
            return if i > 0 && i != usize::MAX { vec![self.nodes[i - 1].0.clone()] } else { Vec::new() };
        }
        self.edges
            .iter()
            .filter(|(from, to, _)| to == id && from != TRIGGER)
            .map(|(from, ..)| from.clone())
            .collect()
    }

    /// The steps whose results reach this one's model: its parents, a
    /// `condition` or `wait` parent standing aside for its own parents
    /// (the engine's `result_parents`).
    fn result_parents(&self, id: &str) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut found = Vec::new();
        let mut queue = self.parents(id);
        while let Some(p) = queue.pop() {
            if p == id || !seen.insert(p.clone()) {
                continue;
            }
            if matches!(self.kind_of(&p), "condition" | "wait") {
                queue.extend(self.parents(&p));
            } else {
                found.push(p);
            }
        }
        found
    }

    /// Every step with a path into this one.
    fn upstream(&self, id: &str) -> Vec<String> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut queue = self.parents(id);
        while let Some(p) = queue.pop() {
            if p == id || !seen.insert(p.clone()) {
                continue;
            }
            queue.extend(self.parents(&p));
        }
        let mut out: Vec<String> = seen.into_iter().collect();
        out.sort_by_key(|n| self.index_of(n));
        out
    }

    /// Whether the step runs inside a loop's body: reachable from a loop's
    /// "Each item" edge before the path returns to that loop.
    fn in_loop_body(&self, id: &str) -> bool {
        self.edges.iter().filter(|(_, _, label)| label.as_deref() == Some("Each item")).any(|(lp, entry, _)| {
            let mut seen = HashSet::new();
            let mut queue = vec![entry.clone()];
            while let Some(n) = queue.pop() {
                if n == *lp || !seen.insert(n.clone()) {
                    continue;
                }
                if n == id {
                    return true;
                }
                queue.extend(self.edges.iter().filter(|(from, ..)| *from == n).map(|(_, to, _)| to.clone()));
            }
            false
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn errors(issues: &[Issue]) -> Vec<String> {
        issues.iter().filter(|i| i.severity == Severity::Error).map(|i| i.to_string()).collect()
    }
    fn warnings(issues: &[Issue]) -> Vec<String> {
        issues.iter().filter(|i| i.severity == Severity::Warning).map(|i| i.to_string()).collect()
    }

    fn act(id: &str) -> Value {
        json!({"id": id, "type": "custom", "intent": format!("do {id}")})
    }
    fn cond(id: &str, expr: &str) -> Value {
        json!({"id": id, "type": "condition", "params": {"expression": expr}})
    }
    fn edge(from: &str, to: &str) -> Value {
        json!({"from": from, "to": to})
    }
    fn branch(from: &str, to: &str, label: &str) -> Value {
        json!({"from": from, "to": to, "label": label})
    }

    #[test]
    fn a_well_wired_workflow_is_clean() {
        let def = json!({
            "activities": [act("classify"), cond("gate", "nodes.classify.ok == true && score >= 3"), act("act")],
            "connections": [edge("__trigger__", "classify"), edge("classify", "gate"), branch("gate", "act", "True")]
        });
        assert_eq!(check_workflow("w", &def), vec![]);
    }

    #[test]
    fn unparseable_condition_is_an_error() {
        let def = json!({
            "activities": [act("a"), cond("c", "reasonCode in savable_reasons")],
            "connections": [edge("__trigger__", "a"), edge("a", "c")]
        });
        let e = errors(&check_workflow("w", &def));
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(e[0].contains("step 'c'") && e[0].contains("'in' is not supported"), "{e:?}");
    }

    #[test]
    fn bare_name_needs_one_parent() {
        let def = json!({
            "activities": [act("a"), act("b"), cond("c", "lane == 'x'")],
            "connections": [edge("__trigger__", "a"), edge("__trigger__", "b"), edge("a", "c"), edge("b", "c")]
        });
        let e = errors(&check_workflow("w", &def));
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(e[0].contains("2 incoming steps"), "{e:?}");
    }

    #[test]
    fn condition_reading_a_non_ancestor_or_missing_node_is_an_error() {
        let def = json!({
            "activities": [act("a"), cond("c", "nodes.later.ok && nodes.ghost.ok"), act("later")],
            "connections": [edge("__trigger__", "a"), edge("a", "c"), branch("c", "later", "True")]
        });
        let e = errors(&check_workflow("w", &def));
        assert_eq!(e.len(), 2, "{e:?}");
        assert!(e.iter().any(|m| m.contains("'later' is not upstream")), "{e:?}");
        assert!(e.iter().any(|m| m.contains("no step 'ghost'")), "{e:?}");
    }

    #[test]
    fn dollar_brace_node_refs_in_commands_are_errors() {
        let def = json!({
            "activities": [
                act("shape"),
                {"id": "economics", "type": "command", "params": {
                    "command": "python3 ${NEBO_SKILL_DIR}/e.py --q ${nodes.shape.quantity} --n {{nodes.shape.n}}"}}
            ],
            "connections": [edge("__trigger__", "shape"), edge("shape", "economics")]
        });
        let e = errors(&check_workflow("w", &def));
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(e[0].contains("${nodes.shape.quantity} is never filled in; write {{nodes.shape.quantity}}"), "{e:?}");
    }

    #[test]
    fn command_placeholder_must_read_an_upstream_node() {
        let def = json!({
            "activities": [act("a"), {"id": "run", "type": "command", "params": {"command": "x {{nodes.b.v}}"}}, act("b")],
            "connections": [edge("__trigger__", "a"), edge("a", "run"), edge("run", "b")]
        });
        let e = errors(&check_workflow("w", &def));
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(e[0].contains("'b' is not upstream"), "{e:?}");
    }

    #[test]
    fn unreachable_step_is_an_error() {
        // Support Triage shape: route has no edge in, so it never runs.
        let def = json!({
            "activities": [act("classify"), act("draft"), act("route")],
            "connections": [edge("__trigger__", "classify"), edge("classify", "draft")]
        });
        let e = errors(&check_workflow("w", &def));
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(e[0].contains("step 'route'") && e[0].contains("never runs"), "{e:?}");
    }

    #[test]
    fn prose_reference_to_non_ancestor_warns_and_unknown_errors() {
        let mut report = act("report");
        report["intent"] = json!("Summarise {{nodes.gather}} and nodes.sibling's notes, plus nodes.nobody.");
        let def = json!({
            "activities": [act("gather"), act("sibling"), report],
            "connections": [edge("__trigger__", "gather"), edge("__trigger__", "sibling"), edge("gather", "report")]
        });
        let issues = check_workflow("w", &def);
        let (e, w) = (errors(&issues), warnings(&issues));
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(e[0].contains("no step 'nobody'"), "{e:?}");
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("'sibling' is not upstream"), "{w:?}");
    }

    #[test]
    fn an_ai_step_fed_an_operations_whole_list_warns() {
        let read = json!({"id": "invoices", "type": "operation", "params": {"operation": "ledger.invoice.search"}});
        let def = json!({
            "activities": [read.clone(), {"id": "digest", "intent": "Summarize the invoices"}],
            "connections": [edge("__trigger__", "invoices"), edge("invoices", "digest")]
        });
        let w = warnings(&check_workflow("w", &def));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("step 'digest'") && w[0].contains("Loop over nodes.invoices.records"), "{w:?}");

        // On the sequential path: the step after it, and one naming it.
        let def = json!({"activities": [read.clone(), act("other"), {"id": "digest", "intent": "Total {{nodes.invoices.records}}"}]});
        let w = warnings(&check_workflow("w", &def));
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(w[0].contains("step 'other'") && w[1].contains("step 'digest'"), "{w:?}");

        // A loop body gets one record at a time: no warning.
        let def = json!({
            "activities": [read, {"id": "each", "type": "loop", "params": {"source": "nodes.invoices.records", "concurrency": 2}},
                {"id": "one", "intent": "Check {{item.id}}"}],
            "connections": [edge("__trigger__", "invoices"), edge("invoices", "each"), branch("each", "one", "Each item"), edge("one", "each")]
        });
        assert_eq!(check_workflow("w", &def), vec![]);
    }

    #[test]
    fn loop_without_concurrency_warns() {
        let def = json!({
            "activities": [act("pull"),
                {"id": "each", "type": "loop", "params": {"source": "nodes.pull.items"}},
                act("one")],
            "connections": [edge("__trigger__", "pull"), edge("pull", "each"), branch("each", "one", "Each item"), edge("one", "each")]
        });
        let issues = check_workflow("w", &def);
        assert_eq!(errors(&issues), Vec::<String>::new());
        let w = warnings(&issues);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("concurrency"), "{w:?}");

        let mut fixed = def.clone();
        fixed["activities"][1]["params"]["concurrency"] = json!(1);
        assert_eq!(check_workflow("w", &fixed), vec![]);
    }

    #[test]
    fn bad_mode_and_regex_are_errors() {
        let def = json!({
            "activities": [act("a"),
                {"id": "c1", "type": "condition", "params": {"expression": "x", "mode": "fuzzy"}},
                {"id": "c2", "type": "condition", "params": {"expression": "(unclosed", "mode": "regex"}}],
            "connections": [edge("__trigger__", "a"), edge("a", "c1"), edge("a", "c2")]
        });
        let e = errors(&check_workflow("w", &def));
        assert_eq!(e.len(), 2, "{e:?}");
    }

    #[test]
    fn agent_config_checks_every_standard_binding() {
        let cfg = crate::agent::parse_agent_config(
            &json!({"workflows": {
                "good": {"trigger": {"type": "manual"}, "activities": [act("a")]},
                "bad": {"trigger": {"type": "manual"},
                    "activities": [act("a"), cond("c", "x && && y")],
                    "connections": [edge("__trigger__", "a"), edge("a", "c")]}
            }})
            .to_string(),
        )
        .unwrap();
        let issues = check_agent(&cfg);
        let text = error_text(&issues).unwrap();
        assert!(text.contains("workflow 'bad', step 'c'"), "{text}");
        assert!(!text.contains("'good'"), "{text}");
    }

    #[test]
    fn trigger_node_ref_points_at_inputs() {
        let def = json!({
            "activities": [{"id": "run", "type": "command", "params": {"command": "x {{nodes.__trigger__.period}}"}}],
            "connections": [edge("__trigger__", "run")]
        });
        let e = errors(&check_workflow("w", &def));
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(e[0].contains("inputs.<field>"), "{e:?}");
    }

    #[test]
    fn unquoted_right_hand_word_warns() {
        let def = json!({
            "activities": [act("confirm"), cond("gate", "nodes.confirm.outcome == won")],
            "connections": [edge("__trigger__", "confirm"), edge("confirm", "gate")]
        });
        let issues = check_workflow("w", &def);
        assert_eq!(errors(&issues), Vec::<String>::new());
        let w = warnings(&issues);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("write 'won'"), "{w:?}");
    }

    #[test]
    fn only_new_errors_block_an_edit() {
        let broken = json!({"activities": [act("a"), cond("c", "x and y")],
            "connections": [edge("__trigger__", "a"), edge("a", "c")]});
        let before = check_workflow("w", &broken);
        assert!(new_error_text(&before, &before).is_none());
        let mut worse = broken.clone();
        worse["activities"].as_array_mut().unwrap().push(act("orphan"));
        let text = new_error_text(&before, &check_workflow("w", &worse)).unwrap();
        assert!(text.contains("orphan") && !text.contains("'c'"), "{text}");
    }

    #[test]
    fn prose_refs_parse() {
        assert_eq!(prose_node_refs("use {{nodes.a-b.x}} and nodes.c, not mynodes.d or ${nodes.e}"), ["a-b", "c", "e"]);
        assert_eq!(prose_node_refs("see nodes.end-"), ["end"]);
    }

    // ── The audit ──

    fn catalog(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    struct Given {
        catalog: BTreeSet<String>,
        history: HashMap<String, BTreeSet<String>>,
        skill_tools: HashMap<String, Vec<String>>,
        plugins: Vec<String>,
        picked: HashMap<String, Vec<String>>,
    }

    impl Given {
        fn new(names: &[&str]) -> Self {
            Self {
                catalog: catalog(names),
                history: HashMap::new(),
                skill_tools: HashMap::new(),
                plugins: Vec::new(),
                picked: HashMap::new(),
            }
        }
        fn audit(&self, def: &Value) -> Audit {
            audit_workflow(
                "w",
                def,
                &Signals {
                    catalog: &self.catalog,
                    history: &self.history,
                    skill_tools: &self.skill_tools,
                    plugins: &self.plugins,
                    picked: &self.picked,
                },
            )
        }
    }

    fn tools_of(def: &Value, step: &str) -> Vec<String> {
        def["activities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["id"] == step)
            .and_then(|a| a["tools"].as_array())
            .map(|t| t.iter().map(|x| x.as_str().unwrap().to_string()).collect())
            .unwrap_or_default()
    }

    /// The tools a step's past runs called are its tools; a name this Nebo
    /// no longer has (or a scheduling tool, which the catalog never holds)
    /// is not added.
    #[test]
    fn run_history_declares_a_steps_tools() {
        let mut given = Given::new(&["plugin__quickbooks", "ledger_transaction_list", "recall"]);
        given.history.insert(
            "feed-check".into(),
            ["plugin__quickbooks", "ledger_transaction_list", "recall", "create_schedule", "emit_event"].iter().map(|s| s.to_string()).collect(),
        );
        let def = json!({"activities": [{"id": "feed-check", "intent": "Check the bank feeds."}]});
        let audit = given.audit(&def);
        assert_eq!(tools_of(&audit.definition, "feed-check"), ["ledger_transaction_list", "plugin__quickbooks", "recall"]);
        assert_eq!(audit.steps[0].resolved_by, Some(Source::History));
        assert!(audit.steps[0].added.iter().all(|a| a.source == Source::History));
        assert!(audit.unresolved.is_empty());
    }

    /// The tools a step's words name: a seat's dotted operation is the tool
    /// it is called through, and a call or an underscored name is itself.
    #[test]
    fn explicit_names_declare_a_steps_tools() {
        let given = Given::new(&["ledger_estimate_list", "ledger_invoice_search", "fetch_url", "write_file"]);
        let def = json!({"activities": [{
            "id": "find-unbilled",
            "intent": "Check accepted estimates via accounting.invoice-sender.ledger.estimate.list.",
            "steps": ["accounting.invoice-sender.ledger.invoice.search for drafts", "Read the page with fetch_url(url: \"x\")", "Write it down."]
        }]});
        let audit = given.audit(&def);
        assert_eq!(tools_of(&audit.definition, "find-unbilled"), ["fetch_url", "ledger_estimate_list", "ledger_invoice_search"]);
        assert_eq!(audit.steps[0].resolved_by, Some(Source::Named));
        assert_eq!(named_tools("see accounting.x.ledger.estimate.list.", &|n| n == "ledger_estimate_list"), ["ledger_estimate_list"]);
    }

    /// What the employee declares: a required plugin the step names, a
    /// connected MCP server the step names, and a skill that brings tools.
    #[test]
    fn declared_requirements_declare_a_steps_tools() {
        let mut given = Given::new(&["plugin__quickbooks", "plugin__plaid", "mcp__hubspot__get_deal", "mcp__hubspot__create_note"]);
        given.plugins = vec!["plugin__quickbooks".into(), "plugin__plaid".into()];
        given.skill_tools.insert("bank-recon".into(), vec!["plugin__plaid".into()]);
        let def = json!({"activities": [
            {"id": "sync", "intent": "Pull this week's bills from QuickBooks and the deal from HubSpot with mcp__hubspot__get_deal."},
            {"id": "recon", "intent": "Reconcile the bank.", "skills": ["bank-recon"]}
        ]});
        let audit = given.audit(&def);
        assert_eq!(tools_of(&audit.definition, "sync"), ["plugin__quickbooks", "mcp__hubspot__*"], "the family covers the named tool");
        assert_eq!(audit.steps[0].resolved_by, Some(Source::Named));
        assert!(tools_of(&audit.definition, "recon").is_empty(), "a skill's tools are added when the run starts");
        assert_eq!(audit.steps[1].resolved_by, Some(Source::Declared));
        assert!(audit.unresolved.is_empty());
    }

    /// An open-ended step nothing resolves is left for the shortlist and
    /// the model; their pick (cached) declares it, and an empty pick says it
    /// needs none. A `tools: []` judging step is never touched.
    #[test]
    fn an_open_ended_step_waits_for_the_pick() {
        let mut given = Given::new(&["search_web", "fetch_url"]);
        let def = json!({"activities": [
            {"id": "research", "intent": "Find out what competitors charge."},
            {"id": "judge", "intent": "Decide which is cheapest.", "tools": []}
        ]});
        let audit = given.audit(&def);
        assert_eq!(audit.unresolved, ["research"]);
        assert!(!audit.changed());
        given.picked.insert("research".into(), vec!["search_web".into(), "not_in_catalog".into()]);
        let audit = given.audit(&def);
        assert!(audit.unresolved.is_empty());
        assert_eq!(tools_of(&audit.definition, "research"), ["search_web"]);
        assert_eq!(audit.steps[0].added[0].source, Source::Model);
        assert_eq!(audit.steps.len(), 1, "the judging step is not declared");
        assert_eq!(audit.definition["activities"][1]["tools"], json!([]));
    }

    /// A step that names a grandparent gets its data by reference; a
    /// parent, a step already referenced and a step inside a loop body are
    /// left as they are.
    #[test]
    fn a_non_parent_reference_is_added() {
        let def = json!({
            "activities": [act("find-unbilled"), act("find-stuck"),
                {"id": "brief", "intent": "Lead with the total from find-unbilled, then what find-stuck found."}],
            "connections": [edge("__trigger__", "find-unbilled"), edge("find-unbilled", "find-stuck"), edge("find-stuck", "brief")]
        });
        let audit = Given::new(&[]).audit(&def);
        let intent = audit.definition["activities"][2]["intent"].as_str().unwrap();
        assert!(intent.contains("{{nodes.find-unbilled}}"), "{intent}");
        assert!(!intent.contains("{{nodes.find-stuck}}"), "a parent's data is already there: {intent}");
        assert_eq!(audit.fixes.len(), 1, "{:?}", audit.fixes);
        let again = Given::new(&[]).audit(&audit.definition);
        assert!(again.fixes.is_empty(), "the fix holds: {:?}", again.fixes);
    }

    /// A condition with one obvious reading is rewritten; a bare name fed
    /// by two steps has no single reading and stays reported.
    #[test]
    fn condition_problems_are_fixed_only_when_unambiguous() {
        let def = json!({
            "activities": [act("a"), act("b"),
                cond("gate", "nodes.a.ok = true and nodes.b.count >= 3"),
                cond("bare", "lane == 'x'"),
                {"id": "run", "type": "command", "params": {"command": "x ${nodes.a.id} {{nodes.__trigger__.period}}"}}],
            "connections": [edge("__trigger__", "a"), edge("__trigger__", "b"), edge("a", "gate"), edge("b", "gate"),
                edge("a", "bare"), edge("b", "bare"), edge("a", "run")]
        });
        let audit = Given::new(&[]).audit(&def);
        assert_eq!(audit.definition["activities"][2]["params"]["expression"], "nodes.a.ok == true && nodes.b.count >= 3");
        assert_eq!(audit.definition["activities"][3]["params"]["expression"], "lane == 'x'");
        assert_eq!(audit.definition["activities"][4]["params"]["command"], "x {{nodes.a.id}} {{inputs.period}}");
        let e = errors(&audit.issues);
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(e[0].contains("step 'bare'") && e[0].contains("2 incoming steps"), "{e:?}");
    }

    /// The Invoice Sender's unbilled sweep, as installed on 2026-10-13: no
    /// step declared tools, so "find-unbilled" ran with the runtime set and
    /// paused on find_tools. The audit declares the operations its words
    /// name, and the hash says when it has to look again.
    #[test]
    fn the_invoice_senders_sweep_is_declared() {
        let given = Given::new(&["ledger_estimate_list", "ledger_invoice_search", "ledger_aging_receivable", "find_tools"]);
        let def = json!({
            "activities": [
                {"id": "find-unbilled", "skills": ["unbilled-sweep"], "intent": "Look for work that became billable and never turned into an invoice. Check accepted estimates with no invoice against them via accounting.invoice-sender.ledger.estimate.list.",
                    "steps": ["accounting.invoice-sender.ledger.estimate.list for accepted estimates with no invoice"]},
                {"id": "find-stuck", "intent": "Pull open invoices via accounting.invoice-sender.ledger.invoice.search and the receivable picture via accounting.invoice-sender.ledger.aging.receivable."},
                {"id": "brief", "cmds": ["emit"], "intent": "Write the shortest useful line and emit accounting.invoice-sender.unbilled-ready."}
            ],
            "connections": [edge("__trigger__", "find-unbilled"), edge("find-unbilled", "find-stuck"), edge("find-stuck", "brief"), edge("brief", "__emit__")]
        });
        let audit = given.audit(&def);
        assert_eq!(tools_of(&audit.definition, "find-unbilled"), ["ledger_estimate_list"]);
        assert_eq!(tools_of(&audit.definition, "find-stuck"), ["ledger_invoice_search", "ledger_aging_receivable"]);
        assert!(tools_of(&audit.definition, "brief").is_empty());
        assert!(audit.unresolved.is_empty(), "{:?}", audit.unresolved);
        assert_ne!(definition_hash(&def), definition_hash(&audit.definition));
        let again = given.audit(&audit.definition);
        assert!(!again.changed());
        assert_eq!(definition_hash(&again.definition), definition_hash(&audit.definition));
    }

    /// A step's key ignores its `tools` (what the pick fills in) and follows
    /// its words and the catalog.
    #[test]
    fn a_step_key_follows_its_words_and_the_catalog() {
        let step = json!({"id": "a", "intent": "Research"});
        let mut declared = step.clone();
        declared["tools"] = json!(["search_web"]);
        assert_eq!(step_key(&step, "c1"), step_key(&declared, "c1"));
        assert_ne!(step_key(&step, "c1"), step_key(&step, "c2"));
        assert_ne!(step_key(&step, "c1"), step_key(&json!({"id": "a", "intent": "Research more"}), "c1"));
    }
}
