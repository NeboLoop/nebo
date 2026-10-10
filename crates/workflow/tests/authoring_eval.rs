//! The authoring eval's scorer: every workflow a generated answer creates is
//! validated and checked against the design rules
//! (`tools::workflows::authoring::AUTHORING_RULES`), then each request is
//! scored. `make eval-authoring` generates the answers and runs
//! `authoring_eval_report`; the other tests pin the checker itself and run
//! in CI with no model.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::Path;

use regex::Regex;
use serde_json::{Value, json};

// ── The definition, read as a graph ────────────────────────────────────

const TRIGGER: &str = "__trigger__";
const EMIT: &str = "__emit__";

struct Graph<'a> {
    def: &'a Value,
    acts: Vec<&'a Value>,
    by_id: HashMap<&'a str, &'a Value>,
    /// (from, to, label)
    edges: Vec<(String, String, Option<String>)>,
}

impl<'a> Graph<'a> {
    fn new(def: &'a Value) -> Self {
        let acts: Vec<&Value> = def["activities"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
        let by_id = acts.iter().filter_map(|a| a["id"].as_str().map(|id| (id, *a))).collect();
        let mut edges: Vec<(String, String, Option<String>)> = def["connections"]
            .as_array()
            .map(|c| {
                c.iter()
                    .map(|e| {
                        (
                            e["from"].as_str().unwrap_or("").to_string(),
                            e["to"].as_str().unwrap_or("").to_string(),
                            e["label"].as_str().map(str::to_string),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        // No connections: the engine runs the activities in order.
        if edges.is_empty() {
            let mut prev = TRIGGER.to_string();
            for a in &acts {
                let id = a["id"].as_str().unwrap_or("").to_string();
                edges.push((prev, id.clone(), None));
                prev = id;
            }
        }
        Graph { def, acts, by_id, edges }
    }

    fn wired(&self) -> bool {
        self.def["connections"].as_array().is_some_and(|c| !c.is_empty())
    }

    fn parents(&self, id: &str) -> Vec<&str> {
        self.edges.iter().filter(|(_, t, _)| t == id).map(|(f, _, _)| f.as_str()).collect()
    }

    fn children(&self, id: &str) -> Vec<&str> {
        self.edges.iter().filter(|(f, _, _)| f == id).map(|(_, t, _)| t.as_str()).collect()
    }

    fn ancestors(&self, id: &str) -> HashSet<String> {
        let mut seen = HashSet::new();
        let mut queue: VecDeque<String> = self.parents(id).into_iter().map(str::to_string).collect();
        while let Some(n) = queue.pop_front() {
            if seen.insert(n.clone()) {
                queue.extend(self.parents(&n).into_iter().map(str::to_string));
            }
        }
        seen
    }

    fn reachable(&self) -> HashSet<String> {
        let mut seen = HashSet::new();
        let mut queue = VecDeque::from([TRIGGER.to_string()]);
        while let Some(n) = queue.pop_front() {
            if seen.insert(n.clone()) {
                queue.extend(self.children(&n).into_iter().map(str::to_string));
            }
        }
        seen
    }

    /// Nodes inside a loop's "Each item" body.
    fn loop_body(&self, loop_id: &str) -> HashSet<String> {
        let mut body = HashSet::new();
        let mut queue: VecDeque<String> = self
            .edges
            .iter()
            .filter(|(f, _, l)| f == loop_id && l.as_deref() == Some("Each item"))
            .map(|(_, t, _)| t.clone())
            .collect();
        while let Some(n) = queue.pop_front() {
            if n == loop_id || n == EMIT || !body.insert(n.clone()) {
                continue;
            }
            queue.extend(self.children(&n).into_iter().map(str::to_string));
        }
        body
    }

    /// Unlabeled out-edges per node (forks; condition/loop edges carry labels).
    fn forks(&self) -> Vec<String> {
        let mut out: BTreeMap<&str, usize> = BTreeMap::new();
        for (f, t, l) in &self.edges {
            if l.is_none() && t != EMIT {
                *out.entry(f.as_str()).or_default() += 1;
            }
        }
        out.into_iter().filter(|(_, n)| *n >= 2).map(|(f, _)| f.to_string()).collect()
    }

    fn joins(&self) -> Vec<String> {
        let mut inc: BTreeMap<&str, usize> = BTreeMap::new();
        for (f, t, l) in &self.edges {
            let into_loop = self.by_id.get(t.as_str()).is_some_and(|a| kind(a) == "loop");
            if l.is_none() && f != TRIGGER && !into_loop {
                *inc.entry(t.as_str()).or_default() += 1;
            }
        }
        inc.into_iter().filter(|(t, n)| *n >= 2 && *t != EMIT).map(|(t, _)| t.to_string()).collect()
    }
}

fn kind(a: &Value) -> &str {
    a["type"].as_str().unwrap_or("")
}
fn is_code(a: &Value) -> bool {
    matches!(kind(a), "command" | "http" | "operation")
}
fn is_control(a: &Value) -> bool {
    matches!(kind(a), "condition" | "loop" | "wait" | "decide")
}
fn is_expert(a: &Value) -> bool {
    kind(a) == "expert"
}
fn is_ai(a: &Value) -> bool {
    !is_code(a) && !is_control(a) && !is_expert(a)
}
fn text(a: &Value) -> String {
    let mut t = a["intent"].as_str().unwrap_or("").to_string();
    for s in a["steps"].as_array().into_iter().flatten() {
        t.push('\n');
        t.push_str(s.as_str().unwrap_or(""));
    }
    if !a["params"].is_null() {
        t.push('\n');
        t.push_str(&a["params"].to_string());
    }
    t
}
fn declared(a: &Value) -> Option<Vec<String>> {
    a["tools"].as_array().map(|t| t.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
}
/// A tool that reaches outside the bot.
fn external(tool: &str) -> bool {
    tool.starts_with("plugin__") || tool.starts_with("mcp__") || matches!(tool, "http_request" | "run_command" | "os" | "send_message")
}

// ── The checks ─────────────────────────────────────────────────────────

#[derive(Default)]
struct Context {
    plugins: Vec<String>,
    mcp_servers: Vec<String>,
}

struct Finding {
    check: &'static str,
    detail: String,
}

/// The validator: the engine's own parse, which the save path runs.
/// (When the wiring validator, napp::workflow_check, lands, it joins here.)
fn validate(def: &Value) -> Result<(), String> {
    let mut d = def.clone();
    for (k, v) in [("id", "eval"), ("name", "eval"), ("version", "1")] {
        if d.get(k).is_none() {
            d[k] = json!(v);
        }
    }
    nebo_workflow::parser::parse_workflow(&d.to_string()).map(|_| ()).map_err(|e| e.to_string())
}

/// Every design-rule finding for one definition.
fn check_definition(def: &Value, ctx: &Context) -> Vec<Finding> {
    let g = Graph::new(def);
    let mut f = Vec::new();
    let mut bad = |check: &'static str, detail: String| f.push(Finding { check, detail });

    if let Err(e) = validate(def) {
        bad("valid", e);
    }
    if g.acts.is_empty() {
        bad("valid", "no activities".into());
    }

    let ids: HashSet<&str> = g.by_id.keys().copied().collect();
    let node_ref = Regex::new(r"\{\{\s*nodes\.([A-Za-z0-9_-]+)").unwrap();
    let dollar_data = Regex::new(r"\$\{\s*(nodes|inputs|item)\b").unwrap();
    let wait_retry = Regex::new(r"(?i)\bretr(y|ies|ying)\b|try again|back ?off|reschedul|create_schedule|\bsleep\s+\d|\bwait\s+\d").unwrap();
    let refs = Regex::new(r"\{\{[^}]*\}\}").unwrap();
    let arithmetic = Regex::new(r"(?i)\b(calculate|compute|add up|sum up|sum of|sum the|total up|totals? (the|of|for each)|tally|count (the|how many)|average)\b").unwrap();
    let side_effect = Regex::new(r"(?i)\b(create|update|send|post|pay|delete|write|apply|merge|publish|transfer)\b").unwrap();
    let judgment = Regex::new(r"(?i)\b(decide|judge|assess|evaluate|determine whether|figure out|work out which|choose|pick)\b").unwrap();
    let fixed_file = Regex::new(r#"(?:/tmp/|~/|\$\{NEBO_DATA_DIR\}/|\./)[^\s'"]*\.(?:json|csv|xlsx|txt)"#).unwrap();
    let plugin_bin = Regex::new(r"\$\{plugin\.([A-Z0-9_]+)_BIN\}").unwrap();
    let installed = |slug: &str| ctx.plugins.iter().any(|p| p.eq_ignore_ascii_case(slug) || p.replace('-', "_").eq_ignore_ascii_case(slug));
    let reachable = g.reachable();

    for a in &g.acts {
        let id = a["id"].as_str().unwrap_or("?");
        let t = text(a);
        // Rule 3: every AI action declares its tools.
        if is_ai(a) && declared(a).is_none() {
            bad("tools-declared", format!("{id}: AI action without `tools`"));
        }
        // Rule 4: data passes by explicit {{nodes.…}} references to earlier steps.
        if dollar_data.is_match(&t) {
            bad("data-explicit", format!("{id}: ${{…}} data syntax"));
        }
        let ancestors = g.ancestors(id);
        for cap in node_ref.captures_iter(&t) {
            let r = &cap[1];
            if !ids.contains(r) {
                bad("data-explicit", format!("{id}: reads nodes.{r}, which is no step"));
            } else if !ancestors.contains(r) {
                bad("data-explicit", format!("{id}: reads nodes.{r}, which is not upstream"));
            }
        }
        // Summaries reach an action unasked; data a code step printed does not.
        let fed = g.parents(id).iter().any(|p| g.by_id.get(p).is_some_and(|pa| is_code(pa)));
        // (A code step may read its system afresh, a verify does.)
        if fed && !is_control(a) && !is_code(a) && !t.contains("{{nodes.") && !t.contains("{{item") {
            bad("data-explicit", format!("{id}: names none of the data it is given"));
        }
        // Rule 5: everything reachable.
        if !reachable.contains(id) {
            bad("wired", format!("{id}: not reachable from the trigger"));
        }
        // Rule 2: an action is one unit.
        let steps = a["steps"].as_array().map_or(0, Vec::len);
        if steps > 7 {
            bad("action-shape", format!("{id}: {steps} steps in one action"));
        }
        if is_ai(a) {
            let tools = declared(a).unwrap_or_default();
            if tools.iter().any(|t| matches!(t.as_str(), "run_command" | "os")) {
                bad("action-shape", format!("{id}: code work inside an AI action"));
            }
            let plain = refs.replace_all(&t, "");
            if tools.iter().any(|t| t.starts_with("plugin__") || t.starts_with("mcp__")) && judgment.is_match(&plain) && side_effect.is_match(&plain) {
                bad("action-shape", format!("{id}: judgment and a write in one action"));
            }
        }
        // Rule 1: every number the owner gets comes from a code step.
        if is_ai(a) && arithmetic.is_match(&refs.replace_all(&t, "")) {
            bad("numbers-from-code", format!("{id}: an AI action computes numbers"));
        }
        // Rules 7/8: no waiting, retrying or scheduling.
        if kind(a) == "wait" || wait_retry.is_match(&t) || declared(a).unwrap_or_default().iter().any(|t| t == "create_schedule") {
            bad("no-wait-retry", format!("{id}: waits, retries or schedules"));
        }
        // Rule 10: per-run files.
        for m in fixed_file.find_iter(&t) {
            // mktemp templates, run ids and shell variables make a path per run.
            let per_run = ["{{", "XXX", "%s", "$", "<"].iter().any(|k| m.as_str().contains(k));
            if !per_run {
                bad("per-run-files", format!("{id}: fixed path {}", m.as_str()));
            }
        }
        // Rule 12: access only through what this bot has.
        for tool in declared(a).unwrap_or_default() {
            if let Some(slug) = tool.strip_prefix("plugin__") {
                if !installed(slug) {
                    bad("access", format!("{id}: declares {tool}, not installed"));
                }
            } else if let Some(rest) = tool.strip_prefix("mcp__") {
                let server = rest.split("__").next().unwrap_or("");
                if !ctx.mcp_servers.iter().any(|s| s == server) {
                    bad("access", format!("{id}: declares {tool}, no such MCP server"));
                }
            }
        }
        for cap in plugin_bin.captures_iter(&t) {
            if !installed(&cap[1].to_lowercase()) {
                bad("access", format!("{id}: runs {}, not installed", &cap[0]));
            }
        }
        if is_ai(a) {
            let flat = t.to_lowercase().replace([' ', '-', '_'], "");
            let tools = declared(a).unwrap_or_default();
            // An action that declares no tools and is handed no data can only
            // reach a system it names by guessing.
            let given_data = t.contains("{{nodes.") || t.contains("{{item");
            let reaches_nothing = tools.is_empty() && !given_data;
            for p in ctx.plugins.iter().filter(|_| reaches_nothing) {
                if flat.contains(&p.to_lowercase().replace('-', "")) && !tools.iter().any(|t| t.trim_start_matches("plugin__").eq_ignore_ascii_case(p)) {
                    bad("access", format!("{id}: works in {p} without its tool"));
                }
            }
        }
        // Rule 5: conditions read data with valid syntax and route both ways.
        if kind(a) == "condition" {
            let expr = a["params"]["expression"].as_str().unwrap_or("");
            let opens = expr.matches('(').count();
            let words = Regex::new(r"(?i)\s(and|or|not)\s").unwrap();
            // Today's engine reads one comparison (the && || ! grammar is #744).
            let compound = expr.contains("&&") || expr.contains("||") || Regex::new(r"!\s*[A-Za-z(]").unwrap().is_match(expr);
            if expr.trim().is_empty() || opens != expr.matches(')').count() || words.is_match(&format!(" {expr} ")) || expr.contains("${") || compound {
                bad("wired", format!("{id}: condition expression `{expr}`"));
            }
            if !g.edges.iter().any(|(f, _, l)| f == id && l.is_some()) {
                bad("wired", format!("{id}: condition without True/False edges"));
            }
        }
        // Rule 11: an expert handoff has a timeout and single-reference inputs.
        if is_expert(a) {
            let p = &a["params"];
            if p["timeout"].as_str().unwrap_or("").is_empty() {
                bad("expert-shape", format!("{id}: expert without params.timeout"));
            }
            if p["expert"].as_str().unwrap_or("").is_empty() || p["task"].as_str().unwrap_or("").is_empty() {
                bad("expert-shape", format!("{id}: expert without params.expert and params.task"));
            }
            let single = Regex::new(r"^\{\{[^}]+\}\}$").unwrap();
            for (k, v) in p["input"].as_object().into_iter().flatten() {
                if !v.as_str().is_some_and(|v| single.is_match(v.trim())) {
                    bad("expert-shape", format!("{id}: input.{k} is not one {{{{reference}}}}"));
                }
            }
            if !g.wired() {
                bad("expert-shape", format!("{id}: an expert workflow needs connections"));
            }
        }
        // Rule 6: a loop sets its concurrency; 1 against a shared system.
        if kind(a) == "loop" {
            let body = g.loop_body(id);
            let external_body = body.iter().filter_map(|n| g.by_id.get(n.as_str())).any(|b| {
                is_code(b) || declared(b).unwrap_or_default().iter().any(|t| external(t))
            });
            let experts_only = body.iter().filter_map(|n| g.by_id.get(n.as_str())).all(|b| is_expert(b) || declared(b).is_some_and(|t| t.is_empty()));
            match a["params"]["concurrency"].as_u64().or_else(|| a["params"]["concurrency"].as_str().and_then(|s| s.parse().ok())) {
                None => bad("loop-concurrency", format!("{id}: loop without params.concurrency")),
                Some(c) if c > 1 && external_body && !experts_only => {
                    bad("loop-concurrency", format!("{id}: concurrency {c} against an external system"))
                }
                _ => {}
            }
            if body.is_empty() {
                bad("wired", format!("{id}: loop without an \"Each item\" body"));
            }
        }
    }
    if g.acts.len() > 1 && !g.wired() {
        bad("wired", "several actions and no connections".into());
    }
    for (from, to, _) in &g.edges {
        if (from != TRIGGER && !ids.contains(from.as_str())) || (to != EMIT && !ids.contains(to.as_str())) {
            bad("wired", format!("edge {from} -> {to} names no step"));
        }
    }
    f
}

// ── Requests ───────────────────────────────────────────────────────────

struct Score {
    id: String,
    checks: Vec<(String, bool, String)>,
    /// Saved without a refusal to fix (the tool's validator).
    first_try: bool,
}

impl Score {
    fn passed(&self) -> bool {
        self.checks.iter().all(|(_, ok, _)| *ok)
    }
}

fn score_record(rec: &Value) -> Score {
    let id = rec["id"].as_str().unwrap_or("?").to_string();
    let expect = &rec["expect"];
    let ctx = Context {
        plugins: rec["context"]["plugins"].as_array().into_iter().flatten().filter_map(|p| p.as_str().map(str::to_string)).collect(),
        mcp_servers: rec["context"]["mcp"].as_array().into_iter().flatten().filter_map(|m| m["server"].as_str().map(str::to_string)).collect(),
    };
    let reply = rec["reply"].as_str().unwrap_or("").to_lowercase();
    let first_try = rec["first_try"].as_bool().unwrap_or(true);
    let calls: Vec<&Value> = rec["calls"].as_array().map(|c| c.iter().collect()).unwrap_or_default();
    let defs: Vec<&Value> = rec["defs"].as_array().into_iter().flatten().map(|d| &d["def"]).collect();
    let graphs: Vec<Graph> = defs.iter().map(|d| Graph::new(d)).collect();
    let all_acts = || graphs.iter().flat_map(|g| g.acts.iter().copied());
    let mut checks: Vec<(String, bool, String)> = Vec::new();
    let mut add = |name: &str, ok: bool, detail: String| checks.push((name.to_string(), ok, detail));

    let needs_workflow = expect["ask_connect"].is_null() && expect["suggest_hire"].is_null();
    if needs_workflow {
        add("produced", !defs.is_empty(), if defs.is_empty() { "no workflow created".into() } else { String::new() });
    }

    // The design rules on every workflow it made.
    let mut by_check: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    for check in ["valid", "tools-declared", "data-explicit", "wired", "action-shape", "loop-concurrency", "no-wait-retry", "per-run-files", "access", "expert-shape", "numbers-from-code"] {
        by_check.insert(check, vec![]);
    }
    for d in &defs {
        for finding in check_definition(d, &ctx) {
            by_check.entry(finding.check).or_default().push(finding.detail);
        }
    }
    if !defs.is_empty() {
        for (check, details) in by_check {
            add(check, details.is_empty(), details.join("; "));
        }
    }

    // What this request calls for.
    if expect["bulk"].as_bool() == Some(true) {
        let ok = graphs.iter().any(|g| {
            // What starts the job reads by code; an AI step beside it may only
            // reach inside the bot (memory, files).
            let first_code = g.children(TRIGGER).iter().any(|c| g.by_id.get(c).is_some_and(|a| is_code(a)))
                && g.children(TRIGGER).iter().all(|c| g.by_id.get(c).is_some_and(|a| {
                    is_code(a) || kind(a) == "condition" || (is_ai(a) && !declared(a).unwrap_or_default().iter().any(|t| external(t)))
                }));
            let per_row = g.acts.iter().filter(|a| kind(a) == "loop").any(|l| {
                g.loop_body(l["id"].as_str().unwrap_or("")).iter().filter_map(|n| g.by_id.get(n.as_str())).any(|b| {
                    is_ai(b) && declared(b).unwrap_or_default().iter().any(|t| external(t))
                })
            });
            first_code && !per_row
        });
        add("bulk-shape", ok, if ok { String::new() } else { "does not fetch with code first, or loops AI actions per record".into() });
    }
    if expect["watch"].as_bool() == Some(true) {
        let ai_optional = expect["ai"].as_str() == Some("optional");
        let ok = graphs.iter().any(|g| {
            let first_code = g.children(TRIGGER).iter().all(|c| g.by_id.get(c).is_some_and(|a| is_code(a)));
            let gated = g.acts.iter().filter(|a| is_ai(a)).all(|a| {
                g.ancestors(a["id"].as_str().unwrap_or("")).iter().any(|n| g.by_id.get(n.as_str()).is_some_and(|p| kind(p) == "condition"))
            });
            let has_condition = g.acts.iter().any(|a| kind(a) == "condition");
            first_code && gated && (has_condition || ai_optional)
        });
        add("watch-code-first", ok, if ok { String::new() } else { "does not start with a code check gating the AI".into() });
    }
    if expect["ceiling"].as_bool() == Some(true) {
        let declared_ceiling = rec["ceilings"].as_array().is_some_and(|c| !c.is_empty())
            || Regex::new(r"ceiling|approval list|approvals list|approvals page").unwrap().is_match(&reply);
        let refs = Regex::new(r"\{\{[^}]*\}\}").unwrap();
        let outward = Regex::new(r"(?i)\b(pay|send|post|publish|transfer|refund|charge)\b").unwrap();
        let command_write = all_acts().filter(|a| kind(a) == "command").find(|a| outward.is_match(&refs.replace_all(a["params"]["command"].as_str().unwrap_or(""), "")));
        let ok = declared_ceiling && command_write.is_none();
        let detail = match (declared_ceiling, command_write) {
            (false, _) => "money-moving or outward operation not in the ceiling".to_string(),
            (_, Some(a)) => format!("{}: an outward write in a command step, which cannot wait for approval", a["id"].as_str().unwrap_or("?")),
            _ => String::new(),
        };
        add("approval-ceiling", ok, detail);
    }
    let experts: Vec<String> = all_acts().filter(|a| is_expert(a)).map(|a| a["params"]["expert"].as_str().unwrap_or("").to_lowercase()).collect();
    match &expect["expert"] {
        Value::String(name) => {
            let ok = experts.iter().any(|e| e.contains(&name.to_lowercase()));
            add("expert-used", ok, if ok { String::new() } else { format!("no expert action for {name} (got {experts:?})") });
        }
        Value::Bool(false) => {
            let ok = experts.is_empty() && !all_acts().any(|a| kind(a) == "agent");
            add("expert-not-used", ok, if ok { String::new() } else { format!("handed simple work to {experts:?}") });
        }
        _ => {}
    }
    if let Some(n) = expect["experts_min"].as_u64() {
        let distinct: HashSet<&String> = experts.iter().collect();
        add("experts-count", distinct.len() as u64 >= n, format!("{} distinct experts", distinct.len()));
    }
    if let Some(name) = expect["suggest_hire"].as_str() {
        let hired = calls.iter().any(|c| matches!(c["tool"].as_str(), Some("hire_employee") | Some("install_workflow")));
        let ok = !hired && reply.contains(&name.to_lowercase());
        add("suggest-not-hire", ok, if ok { String::new() } else { format!("hired, or never suggested {name}") });
    }
    if let Some(name) = expect["propose_cross_owner"].as_str() {
        // Cross-bot experts are proposed to the owner, never generated yet.
        let generated = experts.iter().any(|e| e.contains(&name.to_lowercase()));
        let proposed = reply.contains(&name.to_lowercase()) && Regex::new(r"(approv|\bok\b|okay|permission|allow|consent)").unwrap().is_match(&reply);
        let ok = !generated && proposed;
        add("cross-owner-proposed", ok, if ok { String::new() } else if generated { format!("put {name} in the workflow") } else { format!("never proposes {name} for the owner's OK") });
    }
    match expect["fanout"].as_str() {
        Some("fixed") => {
            let ok = graphs.iter().any(|g| !g.forks().is_empty() && !g.joins().is_empty());
            add("fan-out-join", ok, if ok { String::new() } else { "independent parts run in sequence or never join".into() });
        }
        Some("per_item") => {
            let ok = all_acts().any(|a| kind(a) == "loop");
            add("fan-out-per-item", ok, if ok { String::new() } else { "no loop over the items".into() });
        }
        Some("sequence") => {
            // Parallel reads are fine; dependent work must not fork.
            let forks: Vec<String> = graphs
                .iter()
                .flat_map(|g| {
                    g.forks().into_iter().filter(|f| {
                        g.edges.iter().filter(|(from, _, l)| from == f && l.is_none()).any(|(_, to, _)| g.by_id.get(to.as_str()).is_some_and(|a| !is_code(a)))
                    }).collect::<Vec<_>>()
                })
                .collect();
            add("sequence-kept", forks.is_empty(), if forks.is_empty() { String::new() } else { format!("forks dependent work at {forks:?}") });
        }
        _ => {}
    }
    if let Some(any) = expect["tools_any"].as_array() {
        let wanted: Vec<&str> = any.iter().filter_map(|v| v.as_str()).collect();
        let ok = all_acts().any(|a| declared(a).unwrap_or_default().iter().any(|t| wanted.iter().any(|w| t.starts_with(w))));
        add("access-declared", ok, if ok { String::new() } else { format!("declares none of {wanted:?}") });
    }
    if let Some(slug) = expect["ask_connect"].as_str() {
        let ok = reply.contains(slug) && Regex::new(r"(connect|install|plugin)").unwrap().is_match(&reply);
        add("asks-to-connect", ok, if ok { String::new() } else { format!("never asks to connect {slug}") });
    }
    Score { id, checks, first_try }
}

fn report(scores: &[Score], meta: &Value) -> String {
    let passed = scores.iter().filter(|s| s.passed()).count();
    let first = scores.iter().filter(|s| s.first_try).count();
    let mut out = format!(
        "# Authoring eval\n\nModel: {} · Requests: {} · Passed: {passed}/{} · Saved first try: {first}/{}\n\n| Request | Score | Failed checks |\n|---|---|---|\n",
        meta["model"].as_str().unwrap_or("?"),
        scores.len(),
        scores.len(),
        scores.len()
    );
    for s in scores {
        let ok = s.checks.iter().filter(|(_, ok, _)| *ok).count();
        let failed: Vec<String> = s.checks.iter().filter(|(_, ok, _)| !ok).map(|(n, _, d)| format!("{n}: {d}")).collect();
        out.push_str(&format!(
            "| {} {}{} | {ok}/{} | {} |\n",
            if s.passed() { "PASS" } else { "FAIL" },
            s.id,
            if s.first_try { "" } else { " (fixed after a refusal)" },
            s.checks.len(),
            failed.join("<br>").replace('|', "\\|")
        ));
    }
    out
}

/// `make eval-authoring`: scores the run in AUTHORING_EVAL_DIR, writes
/// report.md and scores.json there, and fails unless every request passes.
#[test]
#[ignore = "scores a generated run: make eval-authoring"]
fn authoring_eval_report() {
    let dir = std::env::var("AUTHORING_EVAL_DIR").expect("AUTHORING_EVAL_DIR names the run");
    let dir = Path::new(&dir);
    let meta: Value = std::fs::read_to_string(dir.join("meta.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}));
    let mut files: Vec<_> = std::fs::read_dir(dir.join("gen")).expect("gen/").filter_map(|e| e.ok()).map(|e| e.path()).collect();
    files.sort();
    let scores: Vec<Score> = files
        .iter()
        .map(|p| score_record(&serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()))
        .collect();
    let md = report(&scores, &meta);
    std::fs::write(dir.join("report.md"), &md).unwrap();
    let json_scores: Vec<Value> = scores
        .iter()
        .map(|s| json!({"id": s.id, "passed": s.passed(), "checks": s.checks.iter().map(|(n, ok, d)| json!({"check": n, "ok": ok, "detail": d})).collect::<Vec<_>>()}))
        .collect();
    std::fs::write(dir.join("scores.json"), serde_json::to_string_pretty(&json_scores).unwrap()).unwrap();
    println!("{md}");
    let failed: Vec<&str> = scores.iter().filter(|s| !s.passed()).map(|s| s.id.as_str()).collect();
    assert!(failed.is_empty(), "{} of {} requests failed: {failed:?}", failed.len(), scores.len());
}

/// The generator's stand-in for create_workflow's refusal: the save
/// path's parse over each definition in AUTHORING_CHECK (a JSON list of
/// {name, def}), its errors written to AUTHORING_CHECK.out.
#[test]
#[ignore = "used by scripts/eval-authoring.py"]
fn authoring_eval_check() {
    let path = std::env::var("AUTHORING_CHECK").expect("AUTHORING_CHECK");
    let defs: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let errors: Vec<Value> = defs
        .iter()
        .map(|d| {
            json!(validate(&d["def"]).err())
        })
        .collect();
    std::fs::write(format!("{path}.out"), serde_json::to_string(&errors).unwrap()).unwrap();
}

// ── The checker, pinned (CI, no model) ─────────────────────────────────

fn worked_example() -> Value {
    let ex = tools::workflows::authoring::WORKED_EXAMPLE;
    serde_json::from_str(&ex[ex.find('{').unwrap()..]).unwrap()
}

fn ctx(plugins: &[&str]) -> Context {
    Context { plugins: plugins.iter().map(|p| p.to_string()).collect(), mcp_servers: vec![] }
}

fn findings(def: &Value, plugins: &[&str]) -> Vec<String> {
    check_definition(def, &ctx(plugins)).into_iter().map(|f| format!("{}: {}", f.check, f.detail)).collect()
}

/// The guidance's own example meets every rule it teaches.
#[test]
fn the_worked_example_passes_every_rule() {
    assert_eq!(findings(&worked_example(), &["mail", "ledger"]), Vec::<String>::new());
    let rec = json!({"id": "example", "expect": {"bulk": true, "ceiling": true, "fanout": "fixed"},
        "context": {"plugins": ["mail", "ledger"]}, "calls": [], "reply": "",
        "ceilings": ["ledger.payment.create"], "defs": [{"name": "pay", "def": worked_example()}]});
    let s = score_record(&rec);
    assert!(s.passed(), "{:?}", s.checks.iter().filter(|c| !c.1).collect::<Vec<_>>());
}

/// The anti-pattern: a loop of AI actions calling the system per record.
#[test]
fn a_per_row_ai_loop_fails_the_bulk_shape() {
    let def = json!({"activities": [
        {"id": "list", "type": "command", "params": {"command": "${plugin.ODOO_BIN} orders list --all --json"}},
        {"id": "each", "type": "loop", "params": {"source": "nodes.list.orders", "concurrency": 4}},
        {"id": "place", "tools": ["plugin__odoo"], "intent": "Decide the customer for {{item.row}} and create the order"}
    ], "connections": [
        {"from": "__trigger__", "to": "list"}, {"from": "list", "to": "each"},
        {"from": "each", "to": "place", "label": "Each item"}, {"from": "place", "to": "each"}
    ]});
    let f = findings(&def, &["odoo"]).join("\n");
    assert!(f.contains("loop-concurrency") && f.contains("action-shape"), "{f}");
    let rec = json!({"id": "x", "expect": {"bulk": true}, "context": {"plugins": ["odoo"]}, "calls": [], "reply": "", "defs": [{"name": "x", "def": def}]});
    let s = score_record(&rec);
    assert!(s.checks.iter().any(|c| c.0 == "bulk-shape" && !c.1));
}

/// Undeclared tools, `${nodes…}`, an unreachable step, a loop with no
/// concurrency, retry prose and a fixed file each fail their rule.
#[test]
fn each_rule_breach_is_named() {
    let def = json!({"activities": [
        {"id": "a", "type": "command", "params": {"command": "x > /tmp/out.json"}},
        {"id": "b", "intent": "Summarize ${nodes.a.rows}; retry if it fails"},
        {"id": "c", "type": "loop", "params": {"source": "nodes.a.rows"}},
        {"id": "d", "tools": [], "intent": "Judge {{item}}"},
        {"id": "orphan", "tools": [], "intent": "never runs"}
    ], "connections": [
        {"from": "__trigger__", "to": "a"}, {"from": "a", "to": "b"}, {"from": "b", "to": "c"},
        {"from": "c", "to": "d", "label": "Each item"}
    ]});
    let f = findings(&def, &[]).join("\n");
    for check in ["tools-declared", "data-explicit", "wired", "loop-concurrency", "no-wait-retry", "per-run-files"] {
        assert!(f.contains(check), "missing {check} in:\n{f}");
    }
}

/// A watch that wakes the AI on every tick fails; one gated by a code
/// check and a condition passes.
#[test]
fn a_watch_starts_with_a_code_check() {
    let ungated = json!({"trigger": {"type": "heartbeat", "interval": "5m"}, "activities": [
        {"id": "look", "tools": ["http_request"], "intent": "Check the site and tell me if it is down"}]});
    let gated = json!({"trigger": {"type": "heartbeat", "interval": "5m"}, "activities": [
        {"id": "probe", "type": "http", "params": {"method": "GET", "url": "https://example.com"}},
        {"id": "down", "type": "condition", "params": {"mode": "expression", "expression": "nodes.probe.status != 200"}},
        {"id": "tell", "tools": [], "intent": "Tell the owner the site is down: {{nodes.probe.status}}"}
    ], "connections": [
        {"from": "__trigger__", "to": "probe"}, {"from": "probe", "to": "down"},
        {"from": "down", "to": "tell", "label": "True"}
    ]});
    let rec = |def: Value| json!({"id": "w", "expect": {"watch": true}, "context": {"plugins": []}, "calls": [], "reply": "", "defs": [{"name": "w", "def": def}]});
    assert!(!score_record(&rec(ungated)).passed());
    let s = score_record(&rec(gated));
    assert!(s.passed(), "{:?}", s.checks.iter().filter(|c| !c.1).collect::<Vec<_>>());
}

/// An expert handoff in #745's shape passes; one with no timeout or a
/// composed input fails.
#[test]
fn an_expert_handoff_has_a_timeout_and_single_reference_inputs() {
    let def = |params: Value| json!({"activities": [
        {"id": "fetch", "type": "command", "params": {"command": "x --json"}},
        {"id": "price", "type": "expert", "params": params, "on_error": {"retry": 1, "fallback": "abort"}}
    ], "connections": [{"from": "__trigger__", "to": "fetch"}, {"from": "fetch", "to": "price"}]});
    let good = def(json!({"expert": "ana", "task": "Price the order for {{inputs.customer}}", "input": {"order": "{{nodes.fetch}}"}, "output": {"total": "number"}, "timeout": "4h"}));
    assert!(!findings(&good, &[]).iter().any(|f| f.starts_with("expert-shape")), "{:?}", findings(&good, &[]));
    let bad = def(json!({"expert": "ana", "task": "Price it", "input": {"order": "order {{nodes.fetch}}"}}));
    let f = findings(&bad, &[]).join("\n");
    assert!(f.contains("expert without params.timeout") && f.contains("input.order"), "{f}");
}
