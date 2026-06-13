use crate::WorkflowError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Top-level workflow definition (parsed from workflow.json).
///
/// Triggers are no longer part of workflow.json — they are owned by Agents
/// (via agent.json). Legacy `triggers` fields are silently ignored on parse.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDef {
    pub version: String,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub inputs: HashMap<String, InputParam>,
    pub activities: Vec<Activity>,
    /// Edges between activities (from the visual builder). Empty = execute
    /// `activities` in array order (legacy sequential path).
    #[serde(default)]
    pub connections: Vec<Connection>,
    #[serde(default)]
    pub dependencies: Dependencies,
    #[serde(default)]
    pub budget: Budget,
}

/// An edge in the workflow graph. `from`/`to` reference activity ids or the
/// `__trigger__`/`__emit__` pseudo-nodes; `label` carries the branch name for
/// edges leaving a branching activity ("True", "False", "Each item", "Done").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connection {
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// A declared branch on a branching activity (condition/loop).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Branch {
    pub label: String,
    #[serde(default, rename = "nextId", skip_serializing_if = "Option::is_none")]
    pub next_id: Option<String>,
}

/// Input parameter definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputParam {
    #[serde(rename = "type")]
    pub param_type: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
}

/// A single activity in the workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Activity {
    pub id: String,
    /// Activity type from the builder: custom, research, email, notify, code,
    /// condition, loop, wait, agent, connector, http, transform. Empty = custom.
    #[serde(rename = "type", default)]
    pub activity_type: String,
    /// Natural-language task. Optional — typed nodes (http, wait, condition)
    /// may be fully described by `params`.
    #[serde(default)]
    pub intent: String,
    /// Display label from the builder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Type-specific parameters (expression/mode, source/maxIterations,
    /// method/url/body, duration, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    /// Declared branches for condition/loop activities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub branches: Vec<Branch>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub mcps: Vec<String>,
    #[serde(default)]
    pub cmds: Vec<String>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub steps: Vec<String>,
    #[serde(default)]
    pub token_budget: TokenBudget,
    #[serde(default)]
    pub on_error: OnError,
    /// Minimum iterations before allowing the activity to stop naturally.
    /// When set, forces continuation even on text-only responses.
    #[serde(default)]
    pub min_iterations: u32,
}

/// Token budget for an activity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBudget {
    #[serde(default = "default_token_max")]
    pub max: u32,
}

fn default_token_max() -> u32 {
    4096
}

impl Default for TokenBudget {
    fn default() -> Self {
        Self {
            max: default_token_max(),
        }
    }
}

/// Error handling policy for an activity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnError {
    #[serde(default = "default_retry")]
    pub retry: u32,
    #[serde(default = "default_fallback")]
    pub fallback: Fallback,
}

fn default_retry() -> u32 {
    1
}
fn default_fallback() -> Fallback {
    Fallback::NotifyOwner
}

impl Default for OnError {
    fn default() -> Self {
        Self {
            retry: default_retry(),
            fallback: default_fallback(),
        }
    }
}

/// Fallback strategy when an activity fails after all retries.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fallback {
    NotifyOwner,
    Skip,
    Abort,
}

/// Workflow dependencies — qualified names that must be installed.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Dependencies {
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub workflows: Vec<String>,
}

/// Budget constraints for the entire workflow run.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Budget {
    #[serde(default)]
    pub total_per_run: u32,
    #[serde(default)]
    pub cost_estimate: String,
}

/// Parse a workflow definition from JSON.
pub fn parse_workflow(json_str: &str) -> Result<WorkflowDef, WorkflowError> {
    let def: WorkflowDef =
        serde_json::from_str(json_str).map_err(|e| WorkflowError::Parse(e.to_string()))?;
    validate_workflow(&def)?;
    Ok(def)
}

/// The trigger pseudo-node id used in `connections`.
pub const TRIGGER_NODE: &str = "__trigger__";
/// The emit pseudo-node id used in `connections`.
pub const EMIT_NODE: &str = "__emit__";

/// Branching activity types — the only types whose outgoing edges carry labels.
pub fn is_branching_type(activity_type: &str) -> bool {
    matches!(activity_type, "condition" | "loop")
}

/// Read a string parameter from an activity's params.
pub(crate) fn param_str<'a>(activity: &'a Activity, key: &str) -> &'a str {
    activity
        .params
        .as_ref()
        .and_then(|p| p.get(key))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

/// Parse a wait duration like "30s", "5m", "1h" (bare numbers are seconds).
pub(crate) fn parse_wait_duration(s: &str) -> Option<std::time::Duration> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (value, unit) = match s.char_indices().last() {
        Some((idx, c)) if c.is_ascii_alphabetic() => (&s[..idx], c.to_ascii_lowercase()),
        _ => (s, 's'),
    };
    let value: u64 = value.trim().parse().ok()?;
    if value == 0 {
        return None;
    }
    let secs = match unit {
        's' => value,
        'm' => value * 60,
        'h' => value * 3_600,
        _ => return None,
    };
    Some(std::time::Duration::from_secs(secs))
}

/// Validate a parsed workflow definition.
pub fn validate_workflow(def: &WorkflowDef) -> Result<(), WorkflowError> {
    if def.id.is_empty() {
        return Err(WorkflowError::Validation("workflow id is required".into()));
    }
    if def.name.is_empty() {
        return Err(WorkflowError::Validation(
            "workflow name is required".into(),
        ));
    }
    if def.activities.is_empty() {
        return Err(WorkflowError::Validation(
            "at least one activity is required".into(),
        ));
    }

    // Check activity IDs are unique
    let mut seen = std::collections::HashSet::new();
    for activity in &def.activities {
        if activity.id.is_empty() {
            return Err(WorkflowError::Validation("activity id is required".into()));
        }
        if !seen.insert(&activity.id) {
            return Err(WorkflowError::Validation(format!(
                "duplicate activity id: {}",
                activity.id
            )));
        }
    }

    // Validate budget sums if total_per_run is set
    if def.budget.total_per_run > 0 {
        let sum: u32 = def.activities.iter().map(|a| a.token_budget.max).sum();
        if sum > def.budget.total_per_run {
            return Err(WorkflowError::Validation(format!(
                "activity token budgets sum ({}) exceeds total_per_run ({})",
                sum, def.budget.total_per_run
            )));
        }
    }

    validate_activities(def)?;
    if !def.connections.is_empty() {
        validate_connections(def)?;
    }

    Ok(())
}

/// Per-activity rules. Routing is deterministic — branching activities must
/// carry the parameters the engine evaluates; LLM-driven activities must have
/// something to execute (intent or steps).
fn validate_activities(def: &WorkflowDef) -> Result<(), WorkflowError> {
    let ids: std::collections::HashSet<&str> =
        def.activities.iter().map(|a| a.id.as_str()).collect();

    for activity in &def.activities {
        match activity.activity_type.as_str() {
            "condition" => {
                if param_str(activity, "expression").trim().is_empty() {
                    return Err(WorkflowError::Validation(format!(
                        "condition activity '{}' requires params.expression — \
                         routing is deterministic, never AI-decided",
                        activity.id
                    )));
                }
            }
            "loop" => {
                if param_str(activity, "source").trim().is_empty() {
                    return Err(WorkflowError::Validation(format!(
                        "loop activity '{}' requires params.source (data path to iterate)",
                        activity.id
                    )));
                }
            }
            // Deterministic executors — no intent needed, params are the contract.
            "http" => {
                if param_str(activity, "url").trim().is_empty() {
                    return Err(WorkflowError::Validation(format!(
                        "http activity '{}' requires params.url",
                        activity.id
                    )));
                }
            }
            "wait" => {
                if !param_str(activity, "waitUntil").trim().is_empty() {
                    return Err(WorkflowError::Validation(format!(
                        "wait activity '{}': waitUntil is not supported — \
                         use params.duration, or trigger a chained workflow on the event",
                        activity.id
                    )));
                }
                if parse_wait_duration(param_str(activity, "duration")).is_none() {
                    return Err(WorkflowError::Validation(format!(
                        "wait activity '{}' requires params.duration (e.g. \"30s\", \"5m\", \"1h\")",
                        activity.id
                    )));
                }
            }
            _ => {
                if activity.intent.trim().is_empty() && activity.steps.is_empty() {
                    return Err(WorkflowError::Validation(format!(
                        "activity '{}' requires an intent or steps",
                        activity.id
                    )));
                }
            }
        }

        for branch in &activity.branches {
            if let Some(next) = &branch.next_id {
                if !ids.contains(next.as_str()) {
                    return Err(WorkflowError::Validation(format!(
                        "activity '{}' branch '{}' references unknown activity '{}'",
                        activity.id, branch.label, next
                    )));
                }
            }
        }
    }

    Ok(())
}

/// Compute a loop's body: every node reachable from its "Each item" edges,
/// stopping at the loop node itself and at __emit__. Used by validation and
/// by the graph executor's per-iteration scoping.
pub(crate) fn loop_body_set(
    def: &WorkflowDef,
    loop_id: &str,
) -> std::collections::HashSet<String> {
    let mut body: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut queue: Vec<&str> = def
        .connections
        .iter()
        .filter(|c| c.from == loop_id && c.label.as_deref() == Some("Each item"))
        .map(|c| c.to.as_str())
        .collect();
    while let Some(node) = queue.pop() {
        if node == loop_id || node == EMIT_NODE || !body.insert(node.to_string()) {
            continue;
        }
        for c in def.connections.iter().filter(|c| c.from == node) {
            queue.push(c.to.as_str());
        }
    }
    body
}

/// Graph rules for explicit connections: endpoint refs resolve, edge direction
/// is honored, branch labels only leave branching activities, and the only
/// permitted cycles are loop-body back-edges into their loop activity.
fn validate_connections(def: &WorkflowDef) -> Result<(), WorkflowError> {
    use std::collections::{HashMap, HashSet};

    let by_id: HashMap<&str, &Activity> =
        def.activities.iter().map(|a| (a.id.as_str(), a)).collect();

    let mut seen_edges: HashSet<(&str, &str, &str)> = HashSet::new();
    for conn in &def.connections {
        if conn.from == EMIT_NODE {
            return Err(WorkflowError::Validation(
                "connection cannot originate from __emit__".into(),
            ));
        }
        if conn.to == TRIGGER_NODE {
            return Err(WorkflowError::Validation(
                "connection cannot target __trigger__".into(),
            ));
        }
        if conn.from != TRIGGER_NODE && !by_id.contains_key(conn.from.as_str()) {
            return Err(WorkflowError::Validation(format!(
                "connection references unknown activity '{}'",
                conn.from
            )));
        }
        if conn.to != EMIT_NODE && !by_id.contains_key(conn.to.as_str()) {
            return Err(WorkflowError::Validation(format!(
                "connection references unknown activity '{}'",
                conn.to
            )));
        }
        if !seen_edges.insert((
            conn.from.as_str(),
            conn.to.as_str(),
            conn.label.as_deref().unwrap_or(""),
        )) {
            return Err(WorkflowError::Validation(format!(
                "duplicate connection {} -> {}",
                conn.from, conn.to
            )));
        }

        let source = by_id.get(conn.from.as_str());
        match &conn.label {
            Some(label) => {
                let Some(source) = source else {
                    return Err(WorkflowError::Validation(
                        "trigger edges cannot carry branch labels".into(),
                    ));
                };
                if !is_branching_type(&source.activity_type) {
                    return Err(WorkflowError::Validation(format!(
                        "labeled edge '{}' leaves non-branching activity '{}'",
                        label, source.id
                    )));
                }
                let valid = if source.branches.is_empty() {
                    let canonical: &[&str] = if source.activity_type == "condition" {
                        &["True", "False"]
                    } else {
                        &["Each item", "Done"]
                    };
                    canonical.contains(&label.as_str())
                } else {
                    source.branches.iter().any(|b| &b.label == label)
                };
                if !valid {
                    return Err(WorkflowError::Validation(format!(
                        "unknown branch label '{}' on activity '{}'",
                        label, source.id
                    )));
                }
            }
            None => {
                if let Some(source) = source {
                    if is_branching_type(&source.activity_type) {
                        return Err(WorkflowError::Validation(format!(
                            "edges leaving '{}' ({}) must carry a branch label",
                            source.id, source.activity_type
                        )));
                    }
                }
            }
        }
    }

    // Cycle detection. The only legal cycles are loop bodies: an edge X -> L
    // where L is a loop activity and X is reachable from L's "Each item"
    // branch without passing through L again. Everything else (including a
    // "Done"-side path returning to the loop) is an infinite workflow.
    let mut exempt: HashSet<(String, String)> = HashSet::new();
    for activity in &def.activities {
        if activity.activity_type != "loop" {
            continue;
        }
        let body = loop_body_set(def, &activity.id);
        // Loop bodies must be self-contained: the engine executes them in an
        // isolated per-iteration scope, so a body node entered from outside
        // the loop would have unsound join semantics.
        for conn in &def.connections {
            if body.contains(&conn.to)
                && conn.from != activity.id
                && conn.from != TRIGGER_NODE
                && !body.contains(&conn.from)
            {
                return Err(WorkflowError::Validation(format!(
                    "activity '{}' is inside the body of loop '{}' and may only \
                     be entered via its Each-item branch (edge from '{}')",
                    conn.to, activity.id, conn.from
                )));
            }
            if body.contains(&conn.to) && conn.from == TRIGGER_NODE {
                return Err(WorkflowError::Validation(format!(
                    "activity '{}' is inside the body of loop '{}' and cannot \
                     be a trigger entry point",
                    conn.to, activity.id
                )));
            }
        }
        for node in &body {
            exempt.insert((node.clone(), activity.id.clone()));
        }
    }
    let exempt: HashSet<(&str, &str)> = exempt
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();

    // DFS three-color over activity nodes, skipping exempt loop-back edges.
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for conn in &def.connections {
        if conn.from == TRIGGER_NODE || conn.to == EMIT_NODE {
            continue;
        }
        if exempt.contains(&(conn.from.as_str(), conn.to.as_str())) {
            continue;
        }
        adjacency
            .entry(conn.from.as_str())
            .or_default()
            .push(conn.to.as_str());
    }
    let mut state: HashMap<&str, u8> = HashMap::new(); // 1 = visiting, 2 = done
    fn dfs<'a>(
        node: &'a str,
        adjacency: &HashMap<&'a str, Vec<&'a str>>,
        state: &mut HashMap<&'a str, u8>,
    ) -> bool {
        match state.get(node) {
            Some(1) => return true,
            Some(2) => return false,
            _ => {}
        }
        state.insert(node, 1);
        for next in adjacency.get(node).map(|v| v.as_slice()).unwrap_or(&[]) {
            if dfs(next, adjacency, state) {
                return true;
            }
        }
        state.insert(node, 2);
        false
    }
    for activity in &def.activities {
        if dfs(activity.id.as_str(), &adjacency, &mut state) {
            return Err(WorkflowError::Validation(
                "workflow graph contains a cycle (only a loop's Each-item body may \
                 return to its loop activity)"
                    .into(),
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic_workflow() {
        let json = r#"{
            "version": "1.0",
            "id": "test-wf",
            "name": "Test Workflow",
            "inputs": {},
            "activities": [{
                "id": "step1",
                "intent": "Do something",
                "model": "sonnet",
                "steps": ["Step one"],
                "token_budget": {"max": 1000}
            }],
            "dependencies": {"skills": []},
            "budget": {"total_per_run": 1000, "cost_estimate": "$0.001"}
        }"#;
        let def = parse_workflow(json).unwrap();
        assert_eq!(def.id, "test-wf");
        assert_eq!(def.activities.len(), 1);
    }

    #[test]
    fn test_parse_legacy_triggers_ignored() {
        // Legacy workflow.json with triggers should parse without error
        let json = r#"{
            "version": "1.0",
            "id": "test-wf",
            "name": "Test",
            "triggers": [{"type": "manual"}],
            "activities": [{
                "id": "step1",
                "intent": "Do something"
            }]
        }"#;
        let def = parse_workflow(json).unwrap();
        assert_eq!(def.id, "test-wf");
    }

    #[test]
    fn test_parse_graph_passthrough() {
        // Builder-produced fields survive the parse: type, params, branches,
        // label, connections — and intent is optional for typed nodes.
        let json = r#"{
            "version": "1.0",
            "id": "graph-wf",
            "name": "Graph",
            "activities": [
                {"id": "check", "type": "condition", "label": "Urgent?",
                 "params": {"expression": "subject contains urgent", "mode": "contains"},
                 "branches": [{"label": "True", "nextId": "notify"}, {"label": "False"}]},
                {"id": "notify", "type": "notify", "intent": "Notify the owner"}
            ],
            "connections": [
                {"from": "__trigger__", "to": "check"},
                {"from": "check", "to": "notify", "label": "True"},
                {"from": "notify", "to": "__emit__"}
            ]
        }"#;
        let def = parse_workflow(json).unwrap();
        assert_eq!(def.connections.len(), 3);
        assert_eq!(def.connections[1].label.as_deref(), Some("True"));
        let cond = &def.activities[0];
        assert_eq!(cond.activity_type, "condition");
        assert_eq!(cond.intent, ""); // optional for typed nodes
        assert_eq!(cond.label.as_deref(), Some("Urgent?"));
        assert_eq!(cond.branches.len(), 2);
        assert_eq!(cond.branches[0].next_id.as_deref(), Some("notify"));
        assert_eq!(
            cond.params.as_ref().unwrap()["mode"].as_str(),
            Some("contains")
        );
        // Round-trip: serialization keeps the wire names (type, nextId)
        let ser = serde_json::to_value(&def).unwrap();
        assert_eq!(ser["activities"][0]["type"], "condition");
        assert_eq!(ser["activities"][0]["branches"][0]["nextId"], "notify");
    }

    /// Helper: minimal def with the given activities/connections JSON.
    fn wf(activities: &str, connections: &str) -> Result<WorkflowDef, WorkflowError> {
        parse_workflow(&format!(
            r#"{{"version":"1.0","id":"t","name":"T","activities":{},"connections":{}}}"#,
            activities, connections
        ))
    }

    #[test]
    fn test_validate_graph_rules() {
        // Legal: condition with labeled branches + parallel fork from a plain node.
        assert!(wf(
            r#"[{"id":"a","intent":"x"},
                {"id":"c","type":"condition","params":{"expression":"a contains x","mode":"contains"}},
                {"id":"b","intent":"y"},{"id":"d","intent":"z"}]"#,
            r#"[{"from":"__trigger__","to":"a"},{"from":"a","to":"c"},
                {"from":"c","to":"b","label":"True"},{"from":"c","to":"d","label":"False"},
                {"from":"a","to":"d"},
                {"from":"b","to":"__emit__"},{"from":"d","to":"__emit__"}]"#
        )
        .is_ok());

        // Legal: loop whose Each-item body returns to the loop (the one allowed cycle).
        assert!(wf(
            r#"[{"id":"l","type":"loop","params":{"source":"inputs.items"}},
                {"id":"body","intent":"process"},{"id":"after","intent":"done"}]"#,
            r#"[{"from":"__trigger__","to":"l"},
                {"from":"l","to":"body","label":"Each item"},{"from":"body","to":"l"},
                {"from":"l","to":"after","label":"Done"}]"#
        )
        .is_ok());

        let plain = r#"[{"id":"a","intent":"x"},{"id":"b","intent":"y"}]"#;
        // Edge out of __emit__ / into __trigger__ / dangling ref / duplicate.
        assert!(wf(plain, r#"[{"from":"__emit__","to":"a"}]"#).is_err());
        assert!(wf(plain, r#"[{"from":"a","to":"__trigger__"}]"#).is_err());
        assert!(wf(plain, r#"[{"from":"a","to":"ghost"}]"#).is_err());
        assert!(wf(
            plain,
            r#"[{"from":"a","to":"b"},{"from":"a","to":"b"}]"#
        )
        .is_err());
        // Labeled edge from a non-branching activity; cycle through plain nodes.
        assert!(wf(plain, r#"[{"from":"a","to":"b","label":"True"}]"#).is_err());
        assert!(wf(
            plain,
            r#"[{"from":"a","to":"b"},{"from":"b","to":"a"}]"#
        )
        .is_err());
        // Condition: missing expression, unlabeled outgoing edge, unknown label.
        assert!(wf(
            r#"[{"id":"c","type":"condition"},{"id":"b","intent":"y"}]"#,
            r#"[{"from":"c","to":"b","label":"True"}]"#
        )
        .is_err());
        let cond = r#"[{"id":"c","type":"condition","params":{"expression":"x"}},{"id":"b","intent":"y"}]"#;
        assert!(wf(cond, r#"[{"from":"c","to":"b"}]"#).is_err());
        assert!(wf(cond, r#"[{"from":"c","to":"b","label":"Maybe"}]"#).is_err());
        // Loop without source; Done-side path cycling back to the loop.
        assert!(wf(
            r#"[{"id":"l","type":"loop"},{"id":"b","intent":"y"}]"#,
            r#"[{"from":"l","to":"b","label":"Each item"}]"#
        )
        .is_err());
        assert!(wf(
            r#"[{"id":"l","type":"loop","params":{"source":"inputs.items"}},
                {"id":"body","intent":"p"},{"id":"after","intent":"d"}]"#,
            r#"[{"from":"l","to":"body","label":"Each item"},{"from":"body","to":"l"},
                {"from":"l","to":"after","label":"Done"},{"from":"after","to":"l"}]"#
        )
        .is_err());
        // LLM-driven activity with neither intent nor steps.
        assert!(wf(r#"[{"id":"a"}]"#, "[]").is_err());
        // http needs a url; wait needs a parseable duration and rejects waitUntil.
        assert!(wf(r#"[{"id":"h","type":"http"}]"#, "[]").is_err());
        assert!(wf(
            r#"[{"id":"h","type":"http","params":{"url":"https://example.com"}}]"#,
            "[]"
        )
        .is_ok());
        assert!(wf(r#"[{"id":"w","type":"wait"}]"#, "[]").is_err());
        assert!(wf(
            r#"[{"id":"w","type":"wait","params":{"duration":"5m"}}]"#,
            "[]"
        )
        .is_ok());
        assert!(wf(
            r#"[{"id":"w","type":"wait","params":{"duration":"5m","waitUntil":"some.event"}}]"#,
            "[]"
        )
        .is_err());
    }

    #[test]
    fn test_parse_wait_duration() {
        use std::time::Duration;
        assert_eq!(parse_wait_duration("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse_wait_duration("5m"), Some(Duration::from_secs(300)));
        assert_eq!(parse_wait_duration("1h"), Some(Duration::from_secs(3600)));
        assert_eq!(parse_wait_duration("45"), Some(Duration::from_secs(45))); // bare = seconds
        assert_eq!(parse_wait_duration("0s"), None);
        assert_eq!(parse_wait_duration(""), None);
        assert_eq!(parse_wait_duration("soon"), None);
        assert_eq!(parse_wait_duration("5d"), None); // unsupported unit
    }

    #[test]
    fn test_validate_duplicate_activity_id() {
        let json = r#"{
            "version": "1.0",
            "id": "test",
            "name": "Test",
            "activities": [
                {"id": "step1", "intent": "a", "steps": []},
                {"id": "step1", "intent": "b", "steps": []}
            ]
        }"#;
        assert!(parse_workflow(json).is_err());
    }
}
