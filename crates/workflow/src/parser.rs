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
    /// condition, loop, wait, agent, connector, http, command, decide, transform.
    /// Empty = custom.
    #[serde(rename = "type", default)]
    pub activity_type: String,
    /// Natural-language task. Optional — typed nodes (http, wait, condition,
    /// command, decide) may be fully described by `params`.
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
    /// Tools this activity MUST successfully call before it may complete.
    /// An activity whose whole point is an outward effect ("send the email")
    /// otherwise reports success while having sent nothing: the model hits a
    /// tool error, narrates it in prose, and stops — which reads as a clean
    /// run in every log. Naming the tool here turns that into a loud failure.
    /// Empty (the default) keeps the old behaviour. Accepts both spellings:
    /// this struct is snake_case on the wire, but hand-written agent.json and
    /// the builder both reach for camelCase, and a silently-ignored guard is
    /// worse than none (the ballast interfaceBindings incident).
    #[serde(default, alias = "requiresTools")]
    pub requires_tools: Vec<String>,
    /// Explicit tool allowlist for this activity. When non-empty it REPLACES
    /// the referenced-in-text detection in `scoped_activity_tools`: only the
    /// named tools (exact name, or a prefix — `"odoo"` covers every
    /// `odoo.*` tool) plus the always-on `message` tool ship their schemas.
    /// The text sniffing keys on `name(` patterns, so skills that write
    /// dotted tool names in prose silently fall back to the FULL roster —
    /// tens of KB of schemas resent every turn (a 13-chunk Vivid run paid
    /// ~18M input tokens for it). Declaring is deterministic and auditable.
    #[serde(default)]
    pub tools: Vec<String>,
}

/// Token budget for an activity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBudget {
    #[serde(default = "default_token_max")]
    pub max: u32,
}

fn default_token_max() -> u32 {
    // 0 = no per-activity budget. Enforcement is opt-in: budgets only bind
    // when the workflow author declares one. A non-zero default here silently
    // capped every activity that never asked for a budget — invisible while
    // usage metering recorded 0, and failing every run the moment metering
    // was fixed. Behavior must be unchanged until tuned.
    0
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

/// Jev caps a Choice at 255 options and a Score at 10 levels.
const DECIDE_CHOICE_MAX: usize = 255;
const DECIDE_SCORE_MAX: usize = 10;

/// The questions a `decide` activity asks, in the Jev wire shape
/// (`{ "<name>": { "type": "choice"|"score"|"noul", "instructions", "criteria" } }`).
/// Accepts an object or a JSON string (the builder's textarea). The same
/// function validates at parse time and feeds the engine at run time, so a
/// definition that parses is one the node can send; every error names the
/// question.
pub(crate) fn decide_questions(
    activity: &Activity,
) -> Result<std::collections::BTreeMap<String, ai::Question>, String> {
    let id = &activity.id;
    let raw = activity.params.as_ref().and_then(|p| p.get("questions"));
    let questions = match raw {
        Some(serde_json::Value::Object(m)) => m.clone(),
        Some(serde_json::Value::String(text)) => {
            match serde_json::from_str::<serde_json::Value>(text) {
                Ok(serde_json::Value::Object(m)) => m,
                _ => {
                    return Err(format!(
                        "decide activity '{id}': params.questions must be a JSON object of questions"
                    ));
                }
            }
        }
        _ => {
            return Err(format!(
                "decide activity '{id}' requires params.questions (a non-empty object of typed questions)"
            ));
        }
    };
    if questions.is_empty() {
        return Err(format!(
            "decide activity '{id}': params.questions must name at least one question"
        ));
    }

    let mut out = std::collections::BTreeMap::new();
    for (name, raw) in questions {
        if name == "model" || name == "defaulted" {
            return Err(format!(
                "decide activity '{id}': question '{name}' collides with the output's {name} field"
            ));
        }
        let kind = raw
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if !matches!(kind.as_str(), "choice" | "score" | "noul") {
            return Err(format!(
                "decide activity '{id}': question '{name}' has type '{kind}' — expected choice, score or noul"
            ));
        }
        if kind == "noul" && raw.get("criteria").is_some() {
            return Err(format!(
                "decide activity '{id}': noul question '{name}' takes no criteria (it judges one statement)"
            ));
        }
        let question: ai::Question = serde_json::from_value(raw).map_err(|e| {
            format!("decide activity '{id}': question '{name}' is malformed: {e}")
        })?;
        let (instructions, count, range) = match &question {
            ai::Question::Choice { instructions, criteria } => {
                (instructions, Some(criteria.len()), 2..=DECIDE_CHOICE_MAX)
            }
            ai::Question::Score { instructions, criteria } => {
                (instructions, Some(criteria.len()), 2..=DECIDE_SCORE_MAX)
            }
            ai::Question::Noul { instructions } => (instructions, None, 0..=0),
        };
        if instructions.trim().is_empty() {
            return Err(format!(
                "decide activity '{id}': question '{name}' needs instructions"
            ));
        }
        if let Some(n) = count {
            if !range.contains(&n) {
                return Err(format!(
                    "decide activity '{id}': {kind} question '{name}' needs {} to {} criteria, has {n}",
                    range.start(),
                    range.end()
                ));
            }
        }
        out.insert(name, question);
    }
    Ok(out)
}

/// The answers a `decide` activity records when no decision can be had: the
/// typed-decision service is not connected, unreachable, throttled or over
/// its deadline. `params.default` (an object, or a JSON string from the
/// builder's textarea) names the author's safe answer per question: an
/// option for a choice, a level for a score, `true`/`false` for a noul.
///
/// A defaulted answer carries confidence 0, so a downstream condition that
/// checks `.confidence` treats it as unsure. A question with no declared
/// default gets no answer at all: every `.choice == ...` condition on it
/// takes its False edge. Choice options are stored sorted, so there is no
/// "first declared" option to fall back on, and an arbitrary option could
/// route a run down an action branch nobody chose.
pub(crate) fn decide_defaults(
    activity: &Activity,
    questions: &std::collections::BTreeMap<String, ai::Question>,
) -> Result<std::collections::BTreeMap<String, ai::Answer>, String> {
    let id = &activity.id;
    let defaults = match activity.params.as_ref().and_then(|p| p.get("default")) {
        None | Some(serde_json::Value::Null) => return Ok(Default::default()),
        Some(serde_json::Value::String(text)) if text.trim().is_empty() => {
            return Ok(Default::default());
        }
        Some(serde_json::Value::Object(m)) => m.clone(),
        Some(serde_json::Value::String(text)) => {
            match serde_json::from_str::<serde_json::Value>(text) {
                Ok(serde_json::Value::Object(m)) => m,
                _ => {
                    return Err(format!(
                        "decide activity '{id}': params.default must be a JSON object of answers"
                    ));
                }
            }
        }
        Some(_) => {
            return Err(format!(
                "decide activity '{id}': params.default must be a JSON object of answers"
            ));
        }
    };

    let mut out = std::collections::BTreeMap::new();
    for (name, value) in defaults {
        let Some(question) = questions.get(&name) else {
            return Err(format!(
                "decide activity '{id}': params.default names '{name}', which is not a question"
            ));
        };
        let answer = match (question, &value) {
            (ai::Question::Choice { criteria, .. }, serde_json::Value::String(option))
                if criteria.contains_key(option) =>
            {
                ai::Answer {
                    kind: "choice".into(),
                    choice: Some(option.clone()),
                    score: None,
                    noul: None,
                    confidence: Some(0.0),
                    probabilities: Default::default(),
                }
            }
            (ai::Question::Score { criteria, .. }, serde_json::Value::String(level))
                if criteria.contains(level) =>
            {
                let index = criteria.iter().position(|l| l == level).unwrap_or(0);
                ai::Answer {
                    kind: "score".into(),
                    choice: None,
                    score: Some(index as f64),
                    noul: None,
                    confidence: Some(0.0),
                    probabilities: Default::default(),
                }
            }
            (ai::Question::Noul { .. }, serde_json::Value::Bool(holds)) => ai::Answer {
                kind: "noul".into(),
                choice: None,
                score: None,
                noul: Some(if *holds { 1.0 } else { 0.0 }),
                confidence: None,
                probabilities: Default::default(),
            },
            (ai::Question::Choice { .. }, _) => {
                return Err(format!(
                    "decide activity '{id}': default for '{name}' must be one of its options"
                ));
            }
            (ai::Question::Score { .. }, _) => {
                return Err(format!(
                    "decide activity '{id}': default for '{name}' must be one of its levels"
                ));
            }
            (ai::Question::Noul { .. }, _) => {
                return Err(format!(
                    "decide activity '{id}': default for '{name}' must be true or false"
                ));
            }
        };
        out.insert(name, answer);
    }
    Ok(out)
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
            "command" => {
                if param_str(activity, "command").trim().is_empty() {
                    return Err(WorkflowError::Validation(format!(
                        "command activity '{}' requires params.command (shell command; \
                         stdout becomes the node output)",
                        activity.id
                    )));
                }
            }
            // Typed decision — the questions are the contract; routing on the
            // answer stays in a condition node.
            "decide" => {
                let questions = decide_questions(activity).map_err(WorkflowError::Validation)?;
                decide_defaults(activity, &questions).map_err(WorkflowError::Validation)?;
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
    fn test_undeclared_token_budget_is_unlimited() {
        // Budgets are opt-in: an activity that declares none gets max 0
        // (uncapped), so enforcement only ever binds where an author asked
        // for it. A silent non-zero default capped every legacy activity the
        // moment usage metering started reporting real numbers.
        let json = r#"{
            "version": "1.0",
            "id": "wf",
            "name": "n",
            "activities": [{"id": "a", "intent": "do"}]
        }"#;
        let def = parse_workflow(json).unwrap();
        assert_eq!(def.activities[0].token_budget.max, 0);
        // Declared budgets still parse and bind.
        let json2 = r#"{
            "version": "1.0",
            "id": "wf2",
            "name": "n",
            "activities": [{"id": "a", "intent": "do", "token_budget": {"max": 2000}}]
        }"#;
        let def2 = parse_workflow(json2).unwrap();
        assert_eq!(def2.activities[0].token_budget.max, 2000);
    }

    /// A one-node decide workflow around the given `questions` JSON.
    fn decide_def(questions: &str) -> String {
        format!(
            r#"{{"version":"1.0","id":"wf","name":"n","activities":[
                {{"id":"classify","type":"decide","params":{{"state":"inputs._event_payload","questions":{questions}}}}}]}}"#
        )
    }

    fn decide_error(questions: &str) -> String {
        match parse_workflow(&decide_def(questions)) {
            Err(WorkflowError::Validation(msg)) => msg,
            other => panic!("expected a validation error, got {other:?}"),
        }
    }

    #[test]
    fn test_decide_accepts_every_question_kind() {
        let def = parse_workflow(&decide_def(
            r#"{
                "intent":{"type":"choice","instructions":"What `text` asks for","criteria":{"quote_request":"a price","other":"anything else"}},
                "urgency":{"type":"score","instructions":"How soon `text` needs an answer","criteria":["routine","soon","now"]},
                "is_spam":{"type":"noul","instructions":"`text` is unsolicited bulk mail"}
            }"#,
        ))
        .expect("valid decide activity");
        assert_eq!(def.activities[0].activity_type, "decide");
        let questions = decide_questions(&def.activities[0]).unwrap();
        assert_eq!(questions.len(), 3);
        assert!(matches!(questions["intent"], ai::Question::Choice { .. }));
        assert!(matches!(questions["urgency"], ai::Question::Score { .. }));
        assert!(matches!(questions["is_spam"], ai::Question::Noul { .. }));
    }

    #[test]
    fn test_decide_accepts_questions_as_json_text() {
        // The builder's textarea saves the object as a string.
        let text = r#"{\"ok\":{\"type\":\"noul\",\"instructions\":\"`text` is fine\"}}"#;
        let def = parse_workflow(&decide_def(&format!("\"{text}\""))).expect("string questions");
        assert_eq!(decide_questions(&def.activities[0]).unwrap().len(), 1);
        assert!(decide_error(r#""not json""#).contains("must be a JSON object"));
    }

    #[test]
    fn test_decide_requires_questions() {
        let def = r#"{"version":"1.0","id":"wf","name":"n","activities":[
            {"id":"classify","type":"decide","params":{"state":"inputs.x"}}]}"#;
        match parse_workflow(def) {
            Err(WorkflowError::Validation(msg)) => {
                assert!(msg.contains("classify") && msg.contains("requires params.questions"), "{msg}")
            }
            other => panic!("expected a validation error, got {other:?}"),
        }
        assert!(decide_error("{}").contains("at least one question"));
    }

    #[test]
    fn test_decide_question_type_and_instructions() {
        let msg = decide_error(r#"{"q":{"type":"rank","instructions":"x","criteria":["a","b"]}}"#);
        assert!(msg.contains("'q'") && msg.contains("rank"), "{msg}");
        let msg = decide_error(r#"{"q":{"instructions":"x"}}"#);
        assert!(msg.contains("'q'") && msg.contains("expected choice, score or noul"), "{msg}");
        let msg = decide_error(r#"{"q":{"type":"noul","instructions":"  "}}"#);
        assert!(msg.contains("'q' needs instructions"), "{msg}");
        let msg = decide_error(r#"{"q":{"type":"noul"}}"#);
        assert!(msg.contains("'q' is malformed"), "{msg}");
        let msg = decide_error(r#"{"model":{"type":"noul","instructions":"x"}}"#);
        assert!(msg.contains("'model' collides"), "{msg}");
        let msg = decide_error(r#"{"defaulted":{"type":"noul","instructions":"x"}}"#);
        assert!(msg.contains("'defaulted' collides"), "{msg}");
    }

    #[test]
    fn test_decide_choice_criteria_bounds() {
        let msg = decide_error(r#"{"q":{"type":"choice","instructions":"x","criteria":{"only":"one"}}}"#);
        assert!(msg.contains("choice question 'q' needs 2 to 255 criteria, has 1"), "{msg}");
        let msg = decide_error(r#"{"q":{"type":"choice","instructions":"x","criteria":["a","b"]}}"#);
        assert!(msg.contains("'q' is malformed"), "{msg}");
        let many: serde_json::Map<String, serde_json::Value> = (0..256)
            .map(|i| (format!("o{i}"), serde_json::Value::String("opt".into())))
            .collect();
        let q = serde_json::json!({"q":{"type":"choice","instructions":"x","criteria":many}});
        let msg = decide_error(&q.to_string());
        assert!(msg.contains("has 256"), "{msg}");
        parse_workflow(&decide_def(
            r#"{"q":{"type":"choice","instructions":"x","criteria":{"a":"1","b":"2"}}}"#,
        ))
        .expect("two options is the floor");
    }

    #[test]
    fn test_decide_score_criteria_bounds() {
        let msg = decide_error(r#"{"q":{"type":"score","instructions":"x","criteria":["one"]}}"#);
        assert!(msg.contains("score question 'q' needs 2 to 10 criteria, has 1"), "{msg}");
        let eleven: Vec<String> = (0..11).map(|i| format!("l{i}")).collect();
        let q = serde_json::json!({"q":{"type":"score","instructions":"x","criteria":eleven}});
        let msg = decide_error(&q.to_string());
        assert!(msg.contains("has 11"), "{msg}");
        let msg = decide_error(r#"{"q":{"type":"score","instructions":"x","criteria":{"a":"b"}}}"#);
        assert!(msg.contains("'q' is malformed"), "{msg}");
        parse_workflow(&decide_def(
            r#"{"q":{"type":"score","instructions":"x","criteria":["low","high"]}}"#,
        ))
        .expect("two levels is the floor");
    }

    #[test]
    fn test_decide_noul_takes_no_criteria() {
        let msg = decide_error(r#"{"q":{"type":"noul","instructions":"x","criteria":["a","b"]}}"#);
        assert!(msg.contains("noul question 'q' takes no criteria"), "{msg}");
        parse_workflow(&decide_def(r#"{"q":{"type":"noul","instructions":"`text` holds"}}"#))
            .expect("a bare noul is valid");
    }

    /// A decide node with the given questions and `params.default`.
    fn decide_def_with_default(questions: &str, default: &str) -> String {
        format!(
            r#"{{"version":"1.0","id":"wf","name":"n","activities":[
                {{"id":"classify","type":"decide","params":{{"state":"inputs.x","questions":{questions},"default":{default}}}}}]}}"#
        )
    }

    #[test]
    fn test_decide_default_answers() {
        let questions = r#"{
            "intent":{"type":"choice","instructions":"x","criteria":{"quote_request":"a price","other":"anything else"}},
            "urgency":{"type":"score","instructions":"x","criteria":["routine","soon","now"]},
            "is_spam":{"type":"noul","instructions":"x"}
        }"#;
        let def = parse_workflow(&decide_def_with_default(
            questions,
            r#"{"intent":"other","urgency":"soon","is_spam":false}"#,
        ))
        .expect("valid defaults");
        let a = &def.activities[0];
        let d = decide_defaults(a, &decide_questions(a).unwrap()).unwrap();
        assert_eq!(d["intent"].picked(), "other");
        assert_eq!(d["intent"].confidence, Some(0.0));
        assert_eq!(d["urgency"].score, Some(1.0));
        assert_eq!(d["is_spam"].noul, Some(0.0));

        // No default: no answers, and that is valid.
        let def = parse_workflow(&decide_def(questions)).unwrap();
        let a = &def.activities[0];
        assert!(
            decide_defaults(a, &decide_questions(a).unwrap())
                .unwrap()
                .is_empty()
        );

        let bad = |default: &str| match parse_workflow(&decide_def_with_default(questions, default))
        {
            Err(WorkflowError::Validation(msg)) => msg,
            other => panic!("expected a validation error, got {other:?}"),
        };
        assert!(bad(r#"{"intent":"refund"}"#).contains("one of its options"));
        assert!(bad(r#"{"urgency":"never"}"#).contains("one of its levels"));
        assert!(bad(r#"{"is_spam":"no"}"#).contains("true or false"));
        assert!(bad(r#"{"mood":"calm"}"#).contains("not a question"));
        assert!(bad(r#"["other"]"#).contains("JSON object of answers"));
    }

    #[test]
    fn test_decide_filter_fixture_validates() {
        let def = parse_workflow(include_str!(
            "../../../tests/fixtures/neboai/decide-filter/workflow.json"
        ))
        .expect("fixture parses and validates");
        assert_eq!(def.id, "decide-filter");
        let classify = def.activities.iter().find(|a| a.id == "classify").unwrap();
        assert_eq!(classify.activity_type, "decide");
        assert_eq!(param_str(classify, "state"), "inputs._event_payload");
        assert!(decide_questions(classify).unwrap().contains_key("intent"));
        assert!(def.activities.iter().any(|a| a.activity_type == "condition"));
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
        // command: params.command is the contract; no intent needed
        assert!(wf(r#"[{"id":"c","type":"command"}]"#, "[]").is_err());
        assert!(wf(
            r#"[{"id":"c","type":"command","params":{"command":"echo hi"}}]"#,
            "[]"
        )
        .is_ok());
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

    /// A self-edge is the smallest illegal cycle — it must be rejected at
    /// validation like any other non-loop-body cycle, never left for the
    /// walker's visit cap to trip on at runtime.
    #[test]
    fn test_validate_rejects_self_edge() {
        let plain = r#"[{"id":"a","intent":"x"}]"#;
        assert!(wf(plain, r#"[{"from":"a","to":"a"}]"#).is_err());
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

#[cfg(test)]
mod requires_tools_tests {
    use super::*;

    /// requiresTools is camelCase on the wire (builder + agent.json) and
    /// defaults to empty, so existing workflows are untouched.
    #[test]
    fn parses_requires_tools_and_defaults_empty() {
        for body in [
            r#"{"id":"converse","requiresTools":["plugin"],"intent":"send it"}"#,
            r#"{"id":"converse","requires_tools":["plugin"],"intent":"send it"}"#,
        ] {
            let a: Activity = serde_json::from_str(body).expect("parse with requires_tools");
            assert_eq!(a.requires_tools, vec!["plugin".to_string()], "body: {body}");
        }

        let without: Activity =
            serde_json::from_str(r#"{"id":"converse","intent":"think"}"#).expect("parse bare");
        assert!(without.requires_tools.is_empty());
    }
}

#[cfg(test)]
mod declared_tools_tests {
    use super::*;

    #[test]
    fn parses_declared_tools_and_defaults_empty() {
        let a: Activity =
            serde_json::from_str(r#"{"id":"resolve","intent":"match rows","tools":["odoo","os"]}"#)
                .expect("parse with tools");
        assert_eq!(a.tools, vec!["odoo".to_string(), "os".to_string()]);

        let b: Activity = serde_json::from_str(r#"{"id":"resolve","intent":"match rows"}"#)
            .expect("parse without tools");
        assert!(b.tools.is_empty());
    }
}
