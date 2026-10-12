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

use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::Value;

use crate::condition::{self, Reference};

const TRIGGER: &str = "__trigger__";
const EMIT: &str = "__emit__";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// One finding about a definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub severity: Severity,
    /// The workflow (binding name or workflow id).
    pub workflow: String,
    /// The step it is about, when it is about one.
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
}
