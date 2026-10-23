//! The workflow audit on an employee's Nebo: it runs
//! `napp::workflow_check::audit_workflow` on each of an employee's
//! workflows with what this Nebo knows, writes the fixes to the workflow,
//! and keeps the report.
//!
//! - **When.** On save and install (in the background, [`Depth::Full`]),
//!   once for every workflow whose definition changed at startup, and
//!   before each run ([`pre_run`]): a hash compare and whether the step's
//!   plugins and MCP servers are still here, which spends no tokens.
//! - **What it reads.** The tools the steps' past runs called (the
//!   permission check's record), the tools the steps' words name, the
//!   employee's plugins, the steps' skills and connected MCP servers. A step
//!   none of those resolve is open-ended: only a full audit asks a
//!   [`Picker`] (an embedding shortlist, then one fast-model call when the
//!   shortlist is not clear), and the pick is kept by the step's words and
//!   the tool catalog, so the same step never costs a second call.
//! - **The report** is per workflow: what was added or fixed, what is
//!   still wrong, and each tool a step called without declaring it
//!   (`Store::record_step_heal`). It is shown with the workflow
//!   (`GET /agents/{id}/workflows`).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use tracing::{info, warn};

use napp::workflow_check::{self, AddedTool, Fix, Issue, Signals, StepTools};

/// How far an audit goes for a step no programmatic signal resolves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// History, names, declarations and kept picks: no tokens.
    Programmatic,
    /// And, for what is left, the [`Picker`].
    Full,
}

impl Depth {
    fn label(self) -> &'static str {
        match self {
            Depth::Programmatic => "programmatic",
            Depth::Full => "full",
        }
    }
}

/// The tools a step may be given, with what each is for: every tool this
/// Nebo has, but what every step already has (the runtime set, and looking
/// tools up, memory and Nebo's own reads) and the tools that schedule work
/// (an unattended run never may).
pub struct Catalog {
    pub tools: BTreeMap<String, String>,
    /// The hash of the names and their purpose lines: a pick is kept under
    /// it ([`workflow_check::step_key`]).
    pub hash: String,
}

impl Catalog {
    pub fn of(defs: impl IntoIterator<Item = (String, String)>) -> Self {
        let tools: BTreeMap<String, String> = defs
            .into_iter()
            .filter(|(name, _)| {
                !workflow::engine::RUNTIME_TOOLS.contains(&name.as_str())
                    && !tools::workflows::step_tools::ALWAYS_ALLOWED.contains(&name.as_str())
                    && !crate::engine::SCHEDULING_TOOLS.contains(&name.as_str())
            })
            .map(|(name, description)| {
                let line = description.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
                (name, line)
            })
            .collect();
        let listing: String = tools.iter().map(|(n, d)| format!("{n}: {d}\n")).collect();
        Self { hash: workflow_check::sha256(&listing), tools }
    }

    /// The registry's tools.
    pub async fn of_registry(registry: &tools::Registry) -> Self {
        Self::of(registry.list().await.into_iter().map(|d| (d.name, d.description)))
    }

    fn names(&self) -> BTreeSet<String> {
        self.tools.keys().cloned().collect()
    }
}

/// What a [`Picker`] chose for one open-ended step.
pub struct Pick {
    pub tools: Vec<String>,
    /// Whether the model was asked (the shortlist alone was not clear).
    pub asked_model: bool,
}

/// Picks the tools of an open-ended step from the catalog.
#[async_trait::async_trait]
pub trait Picker: Send + Sync {
    /// `None`: it could not pick (no embeddings, the model failed); the
    /// step stays unresolved and is tried again next time.
    async fn pick(&self, agent_id: &str, step: &str, catalog: &Catalog) -> Option<Pick>;
}

/// How many tools the embedding shortlist hands the model.
const SHORTLIST: usize = 12;
/// A shortlist is clear without the model when its first tool scores at
/// least this and leads the second by [`CLEAR_LEAD`].
const CLEAR_SCORE: f32 = 0.75;
const CLEAR_LEAD: f32 = 0.15;

/// The picker every Nebo has: Nebo's embedding client for the shortlist,
/// and background work's model (`Housekeeper`, the fast tier) for the pick.
pub struct ModelPicker {
    pub embedder: Arc<dyn ai::EmbeddingProvider>,
    pub housekeeper: agent::harness::model_call::Housekeeper,
}

#[async_trait::async_trait]
impl Picker for ModelPicker {
    async fn pick(&self, agent_id: &str, step: &str, catalog: &Catalog) -> Option<Pick> {
        let entries: Vec<(&String, &String)> = catalog.tools.iter().collect();
        if entries.is_empty() {
            return Some(Pick { tools: Vec::new(), asked_model: false });
        }
        let mut texts: Vec<String> = vec![step.to_string()];
        texts.extend(entries.iter().map(|(n, d)| format!("{n}: {d}")));
        let vectors = self
            .embedder
            .embed(&texts)
            .await
            .map_err(|e| warn!(error = %e, "workflow audit: the shortlist could not be embedded"))
            .ok()?;
        let (query, tools) = vectors.split_first()?;
        let mut scored: Vec<(f32, &str, &str)> = tools
            .iter()
            .zip(&entries)
            .map(|(v, (n, d))| (cosine(query, v), n.as_str(), d.as_str()))
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored.truncate(SHORTLIST);
        if let [first, second, ..] = scored.as_slice()
            && first.0 >= CLEAR_SCORE
            && first.0 - second.0 >= CLEAR_LEAD
        {
            return Some(Pick { tools: vec![first.1.to_string()], asked_model: false });
        }
        let route = self.housekeeper.route("").await?;
        let listing: String = scored.iter().map(|(_, n, d)| format!("- {n}: {d}\n")).collect();
        let req = ai::ChatRequest {
            tool_credential: None,
            caller_waits_for_balance: false,
            chat_id: String::new(),
            ask_channels: None,
            permission_mode: None,
            linked_context: None,
            tool_choice: Default::default(),
            messages: vec![ai::Message {
                role: "user".into(),
                content: format!("Workflow step:\n{step}\n\nTools:\n{listing}"),
                ..Default::default()
            }],
            tools: vec![],
            max_tokens: 200,
            temperature: 0.0,
            system: "Pick the tools this workflow step needs to do its work, only from the list. \
                     Answer with a JSON array of tool names and nothing else; [] when it needs none."
                .into(),
            model: route.model.clone(),
            enable_thinking: false,
            effort: None,
            metadata: None,
            cache_breakpoints: vec![],
            cancel_token: None,
            trace: ai::RequestTrace { agent_id: agent_id.to_string(), ..ai::RequestTrace::new("workflow_audit") },
        };
        let mut rx = route
            .provider
            .stream(&req)
            .await
            .map_err(|e| warn!(error = %e, "workflow audit: the pick failed"))
            .ok()?;
        let mut answer = String::new();
        while let Some(event) = rx.recv().await {
            if event.event_type == ai::StreamEventType::Text {
                answer.push_str(&event.text);
            }
        }
        let shortlist: BTreeSet<&str> = scored.iter().map(|(_, n, _)| *n).collect();
        let tools = parse_pick(&answer, &shortlist)?;
        Some(Pick { tools, asked_model: true })
    }
}

/// The model's answer as tool names, each one from the shortlist; `None`
/// when it is not a JSON array.
fn parse_pick(answer: &str, shortlist: &BTreeSet<&str>) -> Option<Vec<String>> {
    let start = answer.find('[')?;
    let end = answer.rfind(']')?;
    let names: Vec<String> = serde_json::from_str(answer.get(start..=end)?).ok()?;
    let mut out: Vec<String> = Vec::new();
    for name in names {
        if shortlist.contains(name.as_str()) && !out.contains(&name) {
            out.push(name);
        }
    }
    Some(out)
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
    let n = norm(a) * norm(b);
    if n == 0.0 { 0.0 } else { dot / n }
}

/// One workflow's audit, as kept and shown.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub definition_hash: String,
    pub depth: &'static str,
    /// Every AI step: the tools added to it so far and what resolved it.
    pub steps: Vec<StepTools>,
    /// The changes made to steps other than their tools, so far.
    pub fixes: Vec<Fix>,
    /// Open-ended steps nothing resolved yet.
    pub unresolved: Vec<String>,
    /// What is still wrong.
    pub issues: Vec<Issue>,
    /// Tools a step declares that this Nebo doesn't have now (a plugin
    /// removed, an MCP server disconnected).
    pub disconnected: Vec<String>,
    /// Model calls this workflow's audits made.
    pub model_calls: u32,
    /// The engine limits its runs ran into ([`record_run_limit`]): one per
    /// step (`None`: the run as a whole), also listed among `issues`. They
    /// stand while the definition is the one they were met under.
    pub run_limits: Vec<Issue>,
}

/// What one employee needs for its workflows' signals: its plugins and
/// the tools its steps' skills bring.
pub struct Employee {
    pub plugins: Vec<String>,
    pub skill_tools: HashMap<String, Vec<String>>,
}

impl Employee {
    /// Read from the store and the skill loader.
    pub async fn load(store: &db::Store, loader: Option<&tools::skills::Loader>, config: &napp::agent::AgentConfig) -> Self {
        let plugins = config
            .requires
            .plugins
            .iter()
            .filter_map(|p| tools::plugin_tools::plugin_slug_of(store, p))
            .map(|slug| tools::plugin_tools::plugin_tool_name(&slug))
            .collect();
        let mut skill_tools = HashMap::new();
        if let Some(loader) = loader {
            for binding in config.workflows.values() {
                for skill in binding.activities.iter().flat_map(|a| a.skills.iter()) {
                    if skill_tools.contains_key(skill) {
                        continue;
                    }
                    let tools = match loader.get(skill, None).await {
                        Some(s) => crate::workflow_manager::skill_tools(&s),
                        None => Vec::new(),
                    };
                    skill_tools.insert(skill.clone(), tools);
                }
            }
        }
        Self { plugins, skill_tools }
    }
}

/// Whether an audit writes what it fixed to the workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Write {
    /// Write the fixes and keep the report.
    Apply,
    /// Report only (the read-only CLI audit).
    DryRun,
}

/// Audit one of an employee's workflows: its binding as agent.json holds
/// it (`binding_val`). With [`Write::Apply`] the fixed binding is written
/// through the one binding write (`apply_workflow_binding`) unless the
/// workflow changed while it was audited, and the report is kept.
#[allow(clippy::too_many_arguments)]
pub async fn audit_binding(
    store: &db::Store,
    catalog: &Catalog,
    employee: &Employee,
    picker: Option<&dyn Picker>,
    agent_id: &str,
    binding: &str,
    binding_val: &Value,
    depth: Depth,
    write: Write,
) -> Report {
    let history = store.workflow_step_history(agent_id, binding).unwrap_or_default();
    let names = catalog.names();
    let mut picked: HashMap<String, Vec<String>> = HashMap::new();
    for activity in binding_val.get("activities").and_then(Value::as_array).into_iter().flatten() {
        if !workflow_check::is_ai_step(activity) {
            continue;
        }
        let id = activity.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        if let Ok(Some(tools)) = store.step_pick(&workflow_check::step_key(activity, &catalog.hash)) {
            picked.insert(id, tools);
        }
    }
    let signals = |picked: &HashMap<String, Vec<String>>| {
        workflow_check::audit_workflow(
            binding,
            binding_val,
            &Signals {
                catalog: &names,
                history: &history,
                skill_tools: &employee.skill_tools,
                plugins: &employee.plugins,
                picked,
            },
        )
    };
    let mut audit = signals(&picked);
    let mut model_calls = 0;
    if depth == Depth::Full
        && let Some(picker) = picker
        && !audit.unresolved.is_empty()
    {
        for id in audit.unresolved.clone() {
            let Some(activity) = binding_val
                .get("activities")
                .and_then(Value::as_array)
                .and_then(|a| a.iter().find(|x| x.get("id").and_then(Value::as_str) == Some(id.as_str())))
            else {
                continue;
            };
            let Some(pick) = picker.pick(agent_id, &workflow_check::step_text(activity), catalog).await else {
                continue;
            };
            if pick.asked_model {
                model_calls += 1;
            }
            if let Err(e) = store.save_step_pick(&workflow_check::step_key(activity, &catalog.hash), &pick.tools) {
                warn!(error = %e, "workflow audit: the pick was not kept");
            }
            picked.insert(id, pick.tools);
        }
        audit = signals(&picked);
    }

    let registered = |tool: &str| {
        names.contains(tool)
            || tool.strip_suffix('*').is_some_and(|p| names.iter().any(|n| n.starts_with(p)))
    };
    let mut disconnected: Vec<String> = audit
        .definition
        .get("activities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|a| a.get("tools").and_then(Value::as_array).cloned().unwrap_or_default())
        .filter_map(|t| t.as_str().map(str::to_string))
        .filter(|t| (t.starts_with("plugin__") || t.starts_with("mcp__")) && !registered(t))
        .collect();
    disconnected.sort();
    disconnected.dedup();

    let hash = workflow_check::definition_hash(&audit.definition);
    let prior = store.workflow_audit(agent_id, binding).ok().flatten();
    let report = merged_report(prior.as_ref(), &audit, &hash, depth, disconnected, model_calls);
    if write == Write::Apply {
        if audit.changed() {
            let now = current_binding(store, agent_id, binding);
            if now.as_ref().map(workflow_check::definition_hash) == Some(workflow_check::definition_hash(binding_val)) {
                if let Err(e) = crate::workflow_manager::apply_workflow_binding(store, agent_id, binding, &audit.definition, false) {
                    warn!(agent = agent_id, binding, error = %e, "workflow audit: the fixes were not written");
                    return report;
                }
            } else {
                info!(agent = agent_id, binding, "workflow audit: the workflow changed while it was audited; not written");
                return report;
            }
        }
        match serde_json::to_string(&report) {
            Ok(json) => {
                if let Err(e) = store.save_workflow_audit(agent_id, binding, &hash, &json) {
                    warn!(agent = agent_id, binding, error = %e, "workflow audit: the report was not kept");
                }
            }
            Err(e) => warn!(error = %e, "workflow audit: the report did not serialize"),
        }
    }
    report
}

/// The report of this audit, holding what earlier audits added and fixed
/// on steps that still exist.
fn merged_report(
    prior: Option<&db::WorkflowAuditRow>,
    audit: &workflow_check::Audit,
    hash: &str,
    depth: Depth,
    disconnected: Vec<String>,
    model_calls: u32,
) -> Report {
    let prior: Value = prior.and_then(|p| serde_json::from_str(&p.report).ok()).unwrap_or(Value::Null);
    let mut steps = audit.steps.clone();
    for step in &mut steps {
        let earlier: Vec<AddedTool> = prior["steps"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| s["step"] == step.step.as_str())
            .flat_map(|s| s["added"].as_array().cloned().unwrap_or_default())
            .filter_map(|a| {
                let source = serde_json::from_value(a["source"].clone()).ok()?;
                Some(AddedTool { tool: a["tool"].as_str()?.to_string(), source })
            })
            .collect();
        for (i, a) in earlier.into_iter().enumerate() {
            if !step.added.iter().any(|x| x.tool == a.tool) {
                step.added.insert(i.min(step.added.len()), a);
            }
        }
    }
    let ids: BTreeSet<String> = audit
        .definition
        .get("activities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|a| a.get("id").and_then(Value::as_str).map(str::to_string))
        .collect();
    let mut fixes: Vec<Fix> = prior["fixes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| Some(Fix { step: f["step"].as_str()?.to_string(), message: f["message"].as_str()?.to_string() }))
        .filter(|f| ids.contains(&f.step))
        .collect();
    for fix in &audit.fixes {
        if !fixes.contains(fix) {
            fixes.push(fix.clone());
        }
    }
    let prior_hash = prior["definitionHash"].as_str().unwrap_or("");
    let run_limits: Vec<Issue> = if prior_hash.is_empty() || prior_hash == hash {
        run_limits_of(&prior).into_iter().filter(|i| i.activity.as_ref().is_none_or(|s| ids.contains(s))).collect()
    } else {
        Vec::new()
    };
    let mut issues = audit.issues.clone();
    issues.extend(run_limits.iter().cloned());
    Report {
        definition_hash: hash.to_string(),
        depth: depth.label(),
        steps,
        fixes,
        unresolved: audit.unresolved.clone(),
        issues,
        disconnected,
        model_calls: prior["modelCalls"].as_u64().unwrap_or(0) as u32 + model_calls,
        run_limits,
    }
}

/// The run limits a kept report holds.
fn run_limits_of(report: &Value) -> Vec<Issue> {
    report["runLimits"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| {
            Some(Issue {
                severity: workflow_check::Severity::Warning,
                workflow: i["workflow"].as_str()?.to_string(),
                activity: i["step"].as_str().map(str::to_string),
                message: i["message"].as_str()?.to_string(),
            })
        })
        .collect()
}

/// A run of an employee's workflow ran into one of the engine's limits: a
/// step used every model turn it had, or the run used every turn a run
/// has. That is the workflow's shape, not something the employee
/// learned, so it is kept in the workflow's audit report (one finding per
/// step, the newest words), where the owner sees what is wrong with the
/// workflow — never proposed as a learned skill. Before this, every such
/// run filed another "wants to learn something new" item about token
/// budgets or splitting steps (fourteen of them, 2026-10-01 to 10-14).
pub fn record_run_limit(store: &db::Store, agent_id: &str, binding: &str, step: Option<&str>, message: &str) {
    let prior = store.workflow_audit(agent_id, binding).ok().flatten();
    let mut report: Value = prior.as_ref().and_then(|p| serde_json::from_str(&p.report).ok()).unwrap_or_else(|| {
        serde_json::json!({
            "definitionHash": "", "depth": Depth::Programmatic.label(), "steps": [], "fixes": [],
            "unresolved": [], "issues": [], "disconnected": [], "modelCalls": 0,
        })
    });
    let old = run_limits_of(&report);
    let mut limits: Vec<Issue> = old.iter().filter(|i| i.activity.as_deref() != step).cloned().collect();
    limits.push(Issue {
        severity: workflow_check::Severity::Warning,
        workflow: binding.to_string(),
        activity: step.map(str::to_string),
        message: message.to_string(),
    });
    let issue_json = |i: &Issue| serde_json::to_value(i).unwrap_or(Value::Null);
    let old_json: Vec<Value> = old.iter().map(issue_json).collect();
    let mut issues: Vec<Value> = report["issues"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !old_json.contains(i))
        .collect();
    issues.extend(limits.iter().map(issue_json));
    report["issues"] = Value::Array(issues);
    report["runLimits"] = Value::Array(limits.iter().map(issue_json).collect());
    let hash = prior.as_ref().map(|p| p.definition_hash.clone()).unwrap_or_default();
    if let Err(e) = store.save_workflow_audit(agent_id, binding, &hash, &report.to_string()) {
        warn!(agent = agent_id, binding, error = %e, "workflow audit: the run limit was not kept");
    }
}

/// An employee's workflow binding as agent.json holds it now.
fn current_binding(store: &db::Store, agent_id: &str, binding: &str) -> Option<Value> {
    let agent = store.get_agent(agent_id).ok().flatten()?;
    let fm: Value = serde_json::from_str(&agent.frontmatter).ok()?;
    fm.get("workflows")?.get(binding).cloned()
}

/// The standard workflows of an employee's agent.json, by name. Call
/// trees are phone-line configuration, not engine workflows.
fn workflows_of(frontmatter: &str) -> (Option<napp::agent::AgentConfig>, Vec<(String, Value)>) {
    let config = napp::agent::parse_agent_config(if frontmatter.trim().is_empty() { "{}" } else { frontmatter }).ok();
    let fm: Value = serde_json::from_str(frontmatter).unwrap_or(Value::Null);
    let mut out = Vec::new();
    if let Some(config) = &config {
        let mut names: Vec<&String> = config.workflows.keys().collect();
        names.sort();
        for name in names {
            let binding = &config.workflows[name];
            if binding.is_call_tree() || binding.activities.is_empty() {
                continue;
            }
            if let Some(val) = fm.get("workflows").and_then(|w| w.get(name)) {
                out.push((name.clone(), val.clone()));
            }
        }
    }
    (config, out)
}

/// Audit every workflow of an employee. `only_stale`: skip a workflow whose
/// definition is the one last audited, unless open-ended steps of it wait
/// for a full audit (the startup sweep).
#[allow(clippy::too_many_arguments)]
pub async fn audit_employee(
    store: &db::Store,
    catalog: &Catalog,
    loader: Option<&tools::skills::Loader>,
    picker: Option<&dyn Picker>,
    agent_id: &str,
    depth: Depth,
    write: Write,
    only_stale: bool,
) -> Vec<(String, Report)> {
    let Some(agent) = store.get_agent(agent_id).ok().flatten() else { return Vec::new() };
    let (config, workflows) = workflows_of(&agent.frontmatter);
    let Some(config) = config else { return Vec::new() };
    let employee = Employee::load(store, loader, &config).await;
    let mut out = Vec::new();
    for (name, val) in workflows {
        if only_stale && is_settled(store, agent_id, &name, &val, depth) {
            continue;
        }
        let report = audit_binding(store, catalog, &employee, picker, agent_id, &name, &val, depth, write).await;
        out.push((name, report));
    }
    out
}

/// Whether a workflow's last audit stands: the definition is the one it
/// left, and no open-ended step of it waits for a deeper audit.
fn is_settled(store: &db::Store, agent_id: &str, binding: &str, val: &Value, depth: Depth) -> bool {
    let Some(row) = store.workflow_audit(agent_id, binding).ok().flatten() else { return false };
    if row.definition_hash != workflow_check::definition_hash(val) {
        return false;
    }
    let report: Value = serde_json::from_str(&row.report).unwrap_or(Value::Null);
    let waits = report["unresolved"].as_array().is_some_and(|u| !u.is_empty())
        && report["depth"].as_str() != Some(Depth::Full.label());
    !(waits && depth == Depth::Full)
}

/// The check before a run of an employee's workflow: its definition's hash
/// against the last audit's, and whether the plugins and MCP servers its
/// steps declare are still here. Neither spends a token. When either
/// differs, the workflow is audited at once with what is known (no
/// picker), and the run gets the fixed definition. `definition_json` is
/// the run's definition (`WorkflowBinding::to_workflow_json`).
pub async fn pre_run(
    store: &db::Store,
    catalog: &Catalog,
    loader: Option<&tools::skills::Loader>,
    agent_id: &str,
    binding: &str,
    definition_json: String,
) -> String {
    if agent_id.is_empty() || binding.is_empty() {
        return definition_json;
    }
    let Some(agent) = store.get_agent(agent_id).ok().flatten() else { return definition_json };
    let Ok(config) = napp::agent::parse_agent_config(&agent.frontmatter) else { return definition_json };
    let run_shape = |config: &napp::agent::AgentConfig| -> Option<String> {
        config.workflows.get(binding).map(|wb| wb.to_workflow_json(binding))
    };
    // The run is the employee's workflow as it is now; anything else (a
    // case turn's own definition) is not this audit's.
    let hash_of = |json: &str| serde_json::from_str::<Value>(json).map(|v| workflow_check::definition_hash(&v)).ok();
    if run_shape(&config).as_deref().and_then(hash_of) != hash_of(&definition_json) {
        return definition_json;
    }
    let Some(stored) = current_binding(store, agent_id, binding) else { return definition_json };
    let names = catalog.names();
    let here = |tool: &str| {
        names.contains(tool) || tool.strip_suffix('*').is_some_and(|p| names.iter().any(|n| n.starts_with(p)))
    };
    let all_here = stored
        .get("activities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|a| a.get("tools").and_then(Value::as_array).cloned().unwrap_or_default())
        .filter_map(|t| t.as_str().map(str::to_string))
        .filter(|t| t.starts_with("plugin__") || t.starts_with("mcp__"))
        .all(|t| here(&t));
    let last = store.workflow_audit(agent_id, binding).ok().flatten();
    if all_here && last.is_some_and(|row| row.definition_hash == workflow_check::definition_hash(&stored)) {
        return definition_json;
    }
    let employee = Employee::load(store, loader, &config).await;
    audit_binding(store, catalog, &employee, None, agent_id, binding, &stored, Depth::Programmatic, Write::Apply).await;
    info!(agent = agent_id, binding, "workflow audit: the definition changed since its last audit; audited before the run");
    store
        .get_agent(agent_id)
        .ok()
        .flatten()
        .and_then(|a| napp::agent::parse_agent_config(&a.frontmatter).ok())
        .and_then(|c| run_shape(&c))
        .unwrap_or(definition_json)
}

/// The catalog of the read-only CLI audit, made without starting anything:
/// Nebo's own tools and each installed plugin's (`Registry::register_all`
/// over the data directory's plugins, nothing launched), the operations
/// each installed plugin binds (its manifest), and every tool a run on
/// this Nebo has called (a connected MCP server's included).
pub async fn offline_catalog(store: Arc<db::Store>, data_dir: &std::path::Path) -> Catalog {
    let plugins = Arc::new(napp::plugin::PluginStore::new(
        data_dir.join("nebo").join("plugins"),
        data_dir.join("user").join("plugins"),
        None,
    ));
    let registry = tools::Registry::new(Arc::new(agent::harness::permissions::Check::new(store.clone())));
    registry.set_plugin_store(plugins.clone());
    registry.register_all(store.clone(), tools::orchestrator::new_handle()).await;
    let mut tools: BTreeMap<String, String> =
        registry.list().await.into_iter().map(|d| (d.name, d.description)).collect();
    for (slug, ..) in plugins.list_installed() {
        for operation in plugins.get_manifest(&slug).map(|m| m.interface_bindings.into_keys().collect::<Vec<_>>()).unwrap_or_default() {
            tools.entry(tools::operation_tools::operation_tool_name(&operation)).or_insert(operation);
        }
    }
    for name in store.called_tools().unwrap_or_default() {
        tools.entry(name).or_default();
    }
    Catalog::of(tools)
}

/// One installed employee's workflows for the read-only CLI audit.
pub struct EmployeeAudit {
    pub agent_id: String,
    pub name: String,
    pub workflows: Vec<(String, Report)>,
}

/// The read-only audit of every installed employee (`nebo workflows
/// audit`): nothing is written and no model is asked; a step that would
/// need the model is reported unresolved.
pub async fn dry_run(store: &db::Store, catalog: &Catalog, loader: Option<&tools::skills::Loader>) -> Vec<EmployeeAudit> {
    let mut out = Vec::new();
    for agent in store.list_agents(500, 0).unwrap_or_default() {
        let workflows = audit_employee(store, catalog, loader, None, &agent.id, Depth::Programmatic, Write::DryRun, false).await;
        if !workflows.is_empty() {
            out.push(EmployeeAudit { agent_id: agent.id.clone(), name: agent.name.clone(), workflows });
        }
    }
    out
}

/// The read-only audit of published employees' agent.json files (each
/// `*.json` in `dir`): what each would change and what is still wrong.
/// Their tools are this Nebo's catalog, every catalog operation's tool
/// (`tools::interface_catalog`) and the plugins each requires. Nothing is
/// published.
pub fn audit_manifests(store: &db::Store, dir: &std::path::Path, catalog: &Catalog) -> Result<Vec<EmployeeAudit>, String> {
    let mut names = catalog.names();
    names.extend(tools::interface_catalog::operations().map(tools::operation_tools::operation_tool_name));
    let mut out = Vec::new();
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    for path in files {
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let (config, workflows) = workflows_of(&text);
        let plugins: Vec<String> = config
            .as_ref()
            .map(|c| {
                c.requires
                    .plugins
                    .iter()
                    .filter_map(|p| tools::plugin_tools::plugin_slug_of(store, p))
                    .map(|slug| tools::plugin_tools::plugin_tool_name(&slug))
                    .collect()
            })
            .unwrap_or_default();
        let mut catalog_names = names.clone();
        catalog_names.extend(plugins.iter().cloned());
        let none = HashMap::new();
        let mut reports = Vec::new();
        for (name, val) in workflows {
            let audit = workflow_check::audit_workflow(
                &name,
                &val,
                &Signals { catalog: &catalog_names, history: &none, skill_tools: &HashMap::new(), plugins: &plugins, picked: &HashMap::new() },
            );
            let report = merged_report(None, &audit, &workflow_check::definition_hash(&audit.definition), Depth::Programmatic, Vec::new(), 0);
            reports.push((name, report));
        }
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        out.push(EmployeeAudit { agent_id: stem.clone(), name: stem, workflows: reports });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Counting {
        calls: AtomicU32,
        answer: Vec<String>,
    }

    #[async_trait::async_trait]
    impl Picker for Counting {
        async fn pick(&self, _agent: &str, _step: &str, _catalog: &Catalog) -> Option<Pick> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Some(Pick { tools: self.answer.clone(), asked_model: true })
        }
    }

    fn store() -> (tempfile::TempDir, db::Store) {
        let dir = tempfile::tempdir().unwrap();
        let s = db::Store::new(&dir.path().join("t.db").to_string_lossy()).unwrap();
        (dir, s)
    }

    fn catalog() -> Catalog {
        Catalog::of(
            [
                ("ledger_estimate_list", "List estimates"),
                ("ledger_invoice_search", "Search invoices"),
                ("search_web", "Search the web"),
                ("fetch_url", "Read a page"),
                ("find_tools", "Load deferred tools"),
                ("create_schedule", "Schedule work"),
                ("exit", "End the step"),
            ]
            .map(|(n, d)| (n.to_string(), d.to_string())),
        )
    }

    fn employee(s: &db::Store, id: &str, workflows: Value) {
        let fm = serde_json::json!({"workflows": workflows});
        s.create_agent(id, None, "Invoice Sender", "x", "x", &fm.to_string(), None, None).unwrap();
    }

    fn sweep() -> Value {
        serde_json::json!({"unbilled-sweep": {
            "trigger": {"type": "schedule", "cron": "0 8 * * 1-5"},
            "activities": [
                {"id": "find-unbilled", "intent": "Check accepted estimates via accounting.invoice-sender.ledger.estimate.list."},
                {"id": "research", "intent": "Find out what competitors charge for the same work."}
            ],
            "connections": [{"from": "__trigger__", "to": "find-unbilled"}, {"from": "find-unbilled", "to": "research"}]
        }})
    }

    fn tools_of(s: &db::Store, id: &str, step: &str) -> Vec<String> {
        let fm: Value = serde_json::from_str(&s.get_agent(id).unwrap().unwrap().frontmatter).unwrap();
        fm["workflows"]["unbilled-sweep"]["activities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["id"] == step)
            .and_then(|a| a["tools"].as_array())
            .map(|t| t.iter().map(|x| x.as_str().unwrap().to_string()).collect())
            .unwrap_or_default()
    }

    /// The catalog never holds a scheduling tool or what every step has.
    #[test]
    fn the_catalog_leaves_out_scheduling_and_the_runtime_set() {
        let c = catalog();
        assert!(c.tools.contains_key("search_web"));
        assert!(!c.tools.contains_key("create_schedule") && !c.tools.contains_key("exit") && !c.tools.contains_key("find_tools"));
    }

    /// A full audit asks the picker for the open-ended step once; the pick
    /// is kept, and the next full audit of the same step and catalog asks
    /// nothing.
    #[tokio::test]
    async fn a_kept_pick_means_no_model_call() {
        let (_d, s) = store();
        employee(&s, "emp", sweep());
        let picker = Counting { calls: AtomicU32::new(0), answer: vec!["search_web".into()] };
        let c = catalog();
        let first = audit_employee(&s, &c, None, Some(&picker), "emp", Depth::Full, Write::Apply, false).await;
        assert_eq!(picker.calls.load(Ordering::SeqCst), 1);
        assert_eq!(first[0].1.model_calls, 1);
        assert_eq!(tools_of(&s, "emp", "find-unbilled"), ["ledger_estimate_list"]);
        assert_eq!(tools_of(&s, "emp", "research"), ["search_web"]);

        // The author edits the step's tools away; its words are the same.
        let fm: Value = serde_json::from_str(&s.get_agent("emp").unwrap().unwrap().frontmatter).unwrap();
        let mut binding = fm["workflows"]["unbilled-sweep"].clone();
        binding["activities"][1].as_object_mut().unwrap().remove("tools");
        crate::workflow_manager::apply_workflow_binding(&s, "emp", "unbilled-sweep", &binding, true).unwrap();
        let again = audit_employee(&s, &c, None, Some(&picker), "emp", Depth::Full, Write::Apply, false).await;
        assert_eq!(picker.calls.load(Ordering::SeqCst), 1, "the kept pick answered");
        assert_eq!(tools_of(&s, "emp", "research"), ["search_web"]);
        assert_eq!(again[0].1.model_calls, 1, "the total so far");

        // Settled: the startup sweep skips it.
        let swept = audit_employee(&s, &c, None, Some(&picker), "emp", Depth::Full, Write::Apply, true).await;
        assert!(swept.is_empty());
    }

    /// The check before a run spends no tokens: an unchanged workflow is
    /// one hash compare; a changed one is audited with what is known, and an
    /// open-ended step waits for the full audit.
    #[tokio::test]
    async fn the_pre_run_check_spends_no_tokens() {
        let (_d, s) = store();
        employee(&s, "emp", sweep());
        let c = catalog();
        let def = |s: &db::Store| -> String {
            let agent = s.get_agent("emp").unwrap().unwrap();
            let config = napp::agent::parse_agent_config(&agent.frontmatter).unwrap();
            config.workflows["unbilled-sweep"].to_workflow_json("unbilled-sweep")
        };
        let ran = pre_run(&s, &c, None, "emp", "unbilled-sweep", def(&s)).await;
        let ran: Value = serde_json::from_str(&ran).unwrap();
        assert_eq!(ran["activities"][0]["tools"], serde_json::json!(["ledger_estimate_list"]), "audited before the run");
        assert!(ran["activities"][1].get("tools").is_none(), "the open-ended step waits for the full audit");
        let row = s.workflow_audit("emp", "unbilled-sweep").unwrap().unwrap();
        let report: Value = serde_json::from_str(&row.report).unwrap();
        assert_eq!(report["depth"], "programmatic");
        assert_eq!(report["modelCalls"], 0);
        assert_eq!(report["unresolved"], serde_json::json!(["research"]));

        // Unchanged: nothing is audited again.
        let before = s.workflow_audit("emp", "unbilled-sweep").unwrap().unwrap();
        let same = pre_run(&s, &c, None, "emp", "unbilled-sweep", def(&s)).await;
        assert_eq!(same, def(&s));
        assert_eq!(s.workflow_audit("emp", "unbilled-sweep").unwrap().unwrap(), before);

        // A plugin it declares went away: reported, still no tokens.
        let mut fewer = c.tools.clone();
        fewer.remove("ledger_estimate_list");
        let gone = Catalog::of(fewer);
        let mut binding: Value = serde_json::from_str::<Value>(&s.get_agent("emp").unwrap().unwrap().frontmatter).unwrap()["workflows"]["unbilled-sweep"].clone();
        binding["activities"][0]["tools"] = serde_json::json!(["plugin__quickbooks"]);
        crate::workflow_manager::apply_workflow_binding(&s, "emp", "unbilled-sweep", &binding, true).unwrap();
        let _ = pre_run(&s, &gone, None, "emp", "unbilled-sweep", def(&s)).await;
        let report: Value = serde_json::from_str(&s.workflow_audit("emp", "unbilled-sweep").unwrap().unwrap().report).unwrap();
        assert_eq!(report["disconnected"], serde_json::json!(["plugin__quickbooks"]));
        assert_eq!(report["modelCalls"], 0);
    }

    /// A run or a step that ran out of model turns is kept in
    /// the workflow's audit report, one finding per step, and lasts through
    /// the next audit of the same definition; it is never a learned-skill
    /// proposal.
    #[tokio::test]
    async fn a_run_limit_is_kept_in_the_audit_not_proposed() {
        let (_d, s) = store();
        employee(&s, "emp", sweep());
        let over = workflow::WorkflowError::RunMaxIterations(150);
        let (step, message) = crate::workflow_manager::run_limit(&over).expect("a run out of turns is a run limit");
        assert_eq!(step, None, "the whole run");
        record_run_limit(&s, "emp", "unbilled-sweep", step.as_deref(), &message);
        let turns = workflow::WorkflowError::MaxIterations { activity_id: "research".into(), step: Some((1, 4)), turns: 2 };
        let (step, message) = crate::workflow_manager::run_limit(&turns).unwrap();
        record_run_limit(&s, "emp", "unbilled-sweep", step.as_deref(), &message);
        record_run_limit(&s, "emp", "unbilled-sweep", step.as_deref(), &message);
        assert!(crate::workflow_manager::run_limit(&workflow::WorkflowError::Provider("x".into())).is_none());
        // The owner's spending limit is the owner's rule, not the workflow's shape.
        let capped = workflow::WorkflowError::SpendCapReached { activity_id: "research".into(), spent_cents: 50, cap_cents: 50, partial: String::new() };
        assert!(crate::workflow_manager::run_limit(&capped).is_none());

        let report = || -> Value { serde_json::from_str(&s.workflow_audit("emp", "unbilled-sweep").unwrap().unwrap().report).unwrap() };
        assert_eq!(report()["runLimits"].as_array().unwrap().len(), 2, "one per step");
        assert_eq!(report()["issues"].as_array().unwrap().len(), 2);
        assert!(report()["issues"][0]["message"].as_str().unwrap().contains("150 model turns in total"));

        // The next audit of the same workflow keeps them.
        audit_employee(&s, &catalog(), None, None, "emp", Depth::Programmatic, Write::Apply, false).await;
        assert_eq!(report()["runLimits"].as_array().unwrap().len(), 2);
        assert!(report()["issues"].as_array().unwrap().iter().any(|i| i["step"] == "research"));
        assert!(s.open_pending_writes("emp", "skill", "create").unwrap().is_empty(), "no skill proposal");
    }

    #[test]
    fn a_pick_is_held_to_the_shortlist() {
        let shortlist: BTreeSet<&str> = ["search_web", "fetch_url"].into();
        assert_eq!(parse_pick("Sure: [\"search_web\", \"rm_rf\", \"search_web\"]", &shortlist), Some(vec!["search_web".to_string()]));
        assert_eq!(parse_pick("[]", &shortlist), Some(vec![]));
        assert_eq!(parse_pick("none", &shortlist), None);
    }
}
