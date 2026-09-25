//! The one-time upgrade that moves every stored shape naming tools onto the
//! current tool set (Tools-Rewrite-Design §7.4), by `tools::rename_map`. It
//! runs once per install, at the first start of the build that carries it,
//! before anything loads an employee, a skill or a workflow; afterwards
//! nothing reads an old name and nothing aliases one.
//!
//! | Stored shape | Becomes |
//! |---|---|
//! | Calls written in text: employee instructions (AGENT.md, soul, rules), workflow steps and intents, skills (SKILL.md and their reference files, plugin skills included), scheduled job prompts, heartbeat and profile notes, a live run's definition, a queued task's prompt | the current call (`plugin(resource: "rentcast", action: "exec", …)` → `plugin__rentcast(…)`) |
//! | An employee's `requires.tools` and `scopes.<name>.tools` | the tools that took over each old tool's jobs |
//! | A workflow step's `tools` | the same, by exact name |
//! | A workflow step's `requires_tools` | the successor the step's own text calls |
//! | A call-tree intent's `tools` grant and an API key's tool allowlist | the successors, keeping a scope (`work:x` → `run_workflow:x`) |
//! | An app's `tool:` permissions | `tool:` each successor |
//! | A project's `.nebo/hooks.yaml` `tool` / `resource` / `action` filters | the successor tools' names |
//!
//! A workflow run parked on the old approval card becomes an ask
//! ([`convert_parked_approvals`]). Sealed (paid) marketplace content is
//! read from its signed archive and can't be rewritten here: the publisher's
//! republish carries it. Chat history is history and is never rewritten.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use tools::rename_map::{self, successor_names};
use tracing::{info, warn};
use types::NeboError;

#[cfg(test)]
mod tests;

/// The name the conversion is recorded under.
pub const CONVERSION: &str = "stored_tool_names_v1";

/// The name the parked-approval conversion is recorded under.
pub const PARKED_APPROVALS: &str = "parked_approvals_v1";

/// What the conversion changed, and what it could not.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Report {
    pub changes: Vec<Change>,
    /// Old shapes with no successor, left as they were.
    pub unmoved: Vec<Change>,
}

/// One change: where, what it was, what it is now.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Change {
    pub place: String,
    pub before: String,
    pub after: String,
}

impl Report {
    fn changed(&mut self, place: &str, before: impl Into<String>, after: impl Into<String>) {
        self.changes.push(Change { place: place.to_string(), before: before.into(), after: after.into() });
    }

    fn unmoved(&mut self, place: &str, what: impl Into<String>, why: &str) {
        self.unmoved.push(Change { place: place.to_string(), before: what.into(), after: why.to_string() });
    }
}

/// Run the conversion once. `None` when it already ran.
pub fn upgrade(store: &db::Store, data_dir: &Path) -> Result<Option<Report>, NeboError> {
    if store.upgrade_conversion_done(CONVERSION)? {
        return Ok(None);
    }
    let mut report = Report::default();
    rows(store, &mut report)?;
    for root in ["nebo", "user", "packs", "learned"] {
        files(&data_dir.join(root), &mut report);
    }
    hooks(&project_folders(store, data_dir), &mut report);
    for c in &report.changes {
        info!(place = %c.place, before = %c.before, after = %c.after, "stored tool name moved");
    }
    for c in &report.unmoved {
        warn!(place = %c.place, shape = %c.before, why = %c.after, "stored tool name has no successor; left as it was");
    }
    info!(changes = report.changes.len(), unmoved = report.unmoved.len(), "stored tool names moved onto the current tool set");
    store.record_upgrade_conversion(CONVERSION, &serde_json::to_string(&report).unwrap_or_default())?;
    Ok(Some(report))
}

/// Every database place that names tools.
fn rows(store: &db::Store, report: &mut Report) -> Result<(), NeboError> {
    for place in db::tool_naming_places() {
        for cell in store.tool_naming_cells(place)? {
            let at = format!("{place}[{}]", cell.key);
            let Some(value) = rewrite_cell(place, &cell.value, &at, report) else { continue };
            store.set_tool_naming_cell(place, &cell.key, &value)?;
        }
    }
    Ok(())
}

/// A cell's new value, or `None` when nothing in it changed.
fn rewrite_cell(place: &str, value: &str, at: &str, report: &mut Report) -> Option<String> {
    match place {
        "api_keys.tools" => {
            let entries: Vec<String> = serde_json::from_str(value).ok()?;
            let moved = grant_list(&entries, at, report);
            (moved != entries).then(|| serde_json::to_string(&moved).unwrap_or_default())
        }
        // A binding row stores its activities array itself.
        "agent_workflows.activities" => {
            let mut json = serde_json::json!({ "activities": serde_json::from_str::<Value>(value).ok()? });
            rewrite_json(&mut json, at, report).then(|| json["activities"].to_string())
        }
        "agents.frontmatter" | "workflows.definition" | "engine_runs.definition" | "engine_runs.inputs" => {
            let mut json: Value = serde_json::from_str(value).ok()?;
            rewrite_json(&mut json, at, report).then(|| json.to_string())
        }
        _ => {
            let moved = prose(value, at, report);
            (moved != value).then_some(moved)
        }
    }
}

/// Calls written in text.
fn prose(text: &str, at: &str, report: &mut Report) -> String {
    let r = rename_map::rewrite_text(text);
    for (before, after) in r.moved {
        report.changed(at, before, after);
    }
    for shape in r.unmoved {
        report.unmoved(at, shape, "no current tool does this");
    }
    r.text
}

/// Every call written in a JSON document's strings, then the tool lists it
/// declares where employees, workflows and apps declare them. `true` when
/// anything changed.
fn rewrite_json(v: &mut Value, at: &str, report: &mut Report) -> bool {
    let before = v.clone();
    strings(v, at, report);
    lists(v, at, report);
    *v != before
}

fn strings(v: &mut Value, at: &str, report: &mut Report) {
    match v {
        Value::String(s) => {
            let moved = prose(s, at, report);
            if moved != *s {
                *s = moved;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|i| strings(i, at, report)),
        Value::Object(fields) => fields.values_mut().for_each(|f| strings(f, at, report)),
        _ => {}
    }
}

/// The declared tool lists, wherever they sit in a document: `requires.tools`,
/// `scopes.<name>.tools`, `app.permissions`, and each workflow step's
/// `tools`, `requires_tools` and call-tree grant.
fn lists(v: &mut Value, at: &str, report: &mut Report) {
    match v {
        Value::Array(items) => items.iter_mut().for_each(|i| lists(i, at, report)),
        Value::Object(fields) => {
            if let Some(tools) = fields.get_mut("requires").and_then(|r| r.get_mut("tools")) {
                names_in(tools, "", &format!("{at}.requires.tools"), report);
            }
            if let Some(Value::Object(scopes)) = fields.get_mut("scopes") {
                for (name, scope) in scopes.iter_mut() {
                    if let Some(tools) = scope.get_mut("tools") {
                        names_in(tools, "", &format!("{at}.scopes.{name}.tools"), report);
                    }
                }
            }
            if let Some(Value::Array(perms)) = fields.get_mut("app").and_then(|a| a.get_mut("permissions")) {
                app_permissions(perms, &format!("{at}.app.permissions"), report);
            }
            if let Some(Value::Array(steps)) = fields.get_mut("activities") {
                for step in steps.iter_mut() {
                    activity(step, at, report);
                }
            }
            // An agent_workflows row stores the activities array itself.
            for (_, f) in fields.iter_mut() {
                if !matches!(f, Value::String(_)) {
                    lists(f, at, report);
                }
            }
        }
        _ => {}
    }
}

/// One workflow step: its tool scope, the tools it must land, and a
/// call-tree intent's grant.
fn activity(step: &mut Value, at: &str, report: &mut Report) {
    let Some(obj) = step.as_object_mut() else { return };
    let id = obj.get("id").and_then(Value::as_str).unwrap_or("?").to_string();
    let at = format!("{at}.activities[{id}]");
    let text = step_text(obj);
    if let Some(tools) = obj.get_mut("tools") {
        names_in(tools, &text, &format!("{at}.tools"), report);
    }
    for key in ["requires_tools", "requiresTools"] {
        if let Some(Value::Array(required)) = obj.get_mut(key) {
            required_tools(required, &text, &format!("{at}.{key}"), report);
        }
    }
    let is_intent = obj.get("type").and_then(Value::as_str) == Some("intent");
    if is_intent && let Some(grant) = obj.get_mut("params").and_then(|p| p.get_mut("tools")) {
        let at = format!("{at}.params.tools");
        match grant {
            Value::String(s) => {
                let entries: Vec<String> = s.split(',').map(|e| e.trim().to_string()).filter(|e| !e.is_empty()).collect();
                let moved = grant_list(&entries, &at, report);
                if moved != entries {
                    *s = moved.join(", ");
                }
            }
            Value::Array(items) => {
                let entries: Vec<String> = items.iter().filter_map(Value::as_str).map(str::to_string).collect();
                let moved = grant_list(&entries, &at, report);
                if moved != entries {
                    *items = moved.into_iter().map(Value::String).collect();
                }
            }
            _ => {}
        }
    }
}

/// A step's own words: its intent and steps, where it names the calls it
/// makes.
fn step_text(obj: &serde_json::Map<String, Value>) -> String {
    let mut text = obj.get("intent").and_then(Value::as_str).unwrap_or_default().to_string();
    if let Some(Value::Array(steps)) = obj.get("steps") {
        for s in steps.iter().filter_map(Value::as_str) {
            text.push('\n');
            text.push_str(s);
        }
    }
    text
}

/// A list of exact tool names (an employee's always-loaded tools, a step's
/// scope): each old name becomes the plain names of its successors, and a
/// per-call family (`plugin__*`) the members `text` calls (a step's own
/// words; an employee's list has none, and names its plugins in
/// `requires.plugins`). A scoped successor can't be written as one name.
fn names_in(list: &mut Value, text: &str, at: &str, report: &mut Report) {
    let Value::Array(items) = list else { return };
    let entries: Vec<String> = items.iter().filter_map(Value::as_str).map(str::to_string).collect();
    let mut out: Vec<String> = Vec::new();
    for entry in &entries {
        match successor_names(entry) {
            None => push_unique(&mut out, entry.clone()),
            Some(successors) => {
                let plain: Vec<String> = successors
                    .into_iter()
                    .flat_map(|n| if n.ends_with('*') { calls_of(text, &n) } else { vec![n] })
                    .filter(|n| !n.contains([':', '*']))
                    .collect();
                if plain.is_empty() {
                    report.unmoved(at, entry.clone(), "no current tool does this");
                    continue;
                }
                report.changed(at, entry.clone(), plain.join(", "));
                plain.into_iter().for_each(|n| push_unique(&mut out, n));
            }
        }
    }
    if out != entries {
        *items = out.into_iter().map(Value::String).collect();
    }
}

/// A step's `requires_tools`: each must land a successful call, so an old
/// name becomes the successor the step's own words call (`plugin` →
/// `plugin__stannp`, when the step says `plugin__stannp(…)`), or its only
/// successor. One the step can't settle is dropped and reported, never
/// widened into a list the step would have to call in full.
fn required_tools(required: &mut Vec<Value>, text: &str, at: &str, report: &mut Report) {
    let entries: Vec<String> = required.iter().filter_map(Value::as_str).map(str::to_string).collect();
    let mut out: Vec<String> = Vec::new();
    for entry in &entries {
        let Some(successors) = successor_names(entry) else {
            push_unique(&mut out, entry.clone());
            continue;
        };
        let called: Vec<String> = successors.iter().flat_map(|s| calls_of(text, s)).collect();
        let settled = if !called.is_empty() {
            called
        } else {
            match successors.iter().filter(|n| !n.contains([':', '*'])).collect::<Vec<_>>().as_slice() {
                [only] => vec![(*only).clone()],
                _ => Vec::new(),
            }
        };
        if settled.is_empty() {
            report.unmoved(at, entry.clone(), "the step names none of its successors; the requirement is dropped");
            continue;
        }
        report.changed(at, entry.clone(), settled.join(", "));
        settled.into_iter().for_each(|n| push_unique(&mut out, n));
    }
    if out != entries {
        *required = out.into_iter().map(Value::String).collect();
    }
}

/// The tools `text` calls that `name` covers: `name` itself, or each
/// member of a family (`plugin__*`).
fn calls_of(text: &str, name: &str) -> Vec<String> {
    let base = name.split(':').next().unwrap_or(name);
    match base.strip_suffix('*') {
        None => text.contains(&format!("{base}(")).then(|| base.to_string()).into_iter().collect(),
        Some(prefix) => {
            let mut found = Vec::new();
            let mut rest = text;
            while let Some(i) = rest.find(prefix) {
                let tail = &rest[i + prefix.len()..];
                let end = tail.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-')).unwrap_or(tail.len());
                if end > 0 && tail[end..].starts_with('(') {
                    push_unique(&mut found, format!("{prefix}{}", &tail[..end]));
                }
                rest = &tail[end..];
            }
            found
        }
    }
}

/// A tool allowlist (an API key's, a call-tree intent's): each old entry
/// becomes its successors, scopes and families included.
fn grant_list(entries: &[String], at: &str, report: &mut Report) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for entry in entries {
        match successor_names(entry) {
            None => push_unique(&mut out, entry.clone()),
            Some(successors) if successors.is_empty() => report.unmoved(at, entry.clone(), "no current tool does this"),
            Some(successors) => {
                report.changed(at, entry.clone(), successors.join(", "));
                successors.into_iter().for_each(|n| push_unique(&mut out, n));
            }
        }
    }
    out
}

/// An app's `tool:<name>` permissions.
fn app_permissions(perms: &mut Vec<Value>, at: &str, report: &mut Report) {
    let entries: Vec<String> = perms.iter().filter_map(Value::as_str).map(str::to_string).collect();
    let mut out: Vec<String> = Vec::new();
    for entry in &entries {
        let moved = entry.strip_prefix("tool:").and_then(successor_names);
        match moved {
            None => push_unique(&mut out, entry.clone()),
            Some(successors) => {
                let named: Vec<String> =
                    successors.into_iter().filter(|n| !n.contains([':', '*'])).map(|n| format!("tool:{n}")).collect();
                if named.is_empty() {
                    report.unmoved(at, entry.clone(), "no current tool does this");
                    continue;
                }
                report.changed(at, entry.clone(), named.join(", "));
                named.into_iter().for_each(|n| push_unique(&mut out, n));
            }
        }
    }
    if out != entries {
        *perms = out.into_iter().map(Value::String).collect();
    }
}

fn push_unique(out: &mut Vec<String>, name: String) {
    if !out.contains(&name) {
        out.push(name);
    }
}

/// The unsealed files under `root` that name tools: every Markdown file (a
/// skill, its reference files, an employee's AGENT.md, a pack's layers) and
/// every `agent.json`. Signed manifests are never touched.
fn files(root: &Path, report: &mut Report) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            files(&path, report);
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_md = name.to_lowercase().ends_with(".md");
        if !is_md && name != "agent.json" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let at = path.display().to_string();
        let moved = if is_md {
            let moved = prose(&text, &at, report);
            (moved != text).then_some(moved)
        } else {
            serde_json::from_str::<Value>(&text).ok().and_then(|mut json| {
                rewrite_json(&mut json, &at, report)
                    .then(|| serde_json::to_string_pretty(&json).unwrap_or_default())
            })
        };
        if let Some(moved) = moved
            && let Err(e) = std::fs::write(&path, moved)
        {
            warn!(path = %at, error = %e, "stored tool names: file not rewritten");
        }
    }
}

/// The project folders Nebo knows: every folder rule, and the worktrees it
/// made. A hooks file above one of them (up to its git root) is the one a
/// call there reads.
fn project_folders(store: &db::Store, data_dir: &Path) -> Vec<PathBuf> {
    let mut folders: BTreeSet<PathBuf> = BTreeSet::new();
    let mut scopes = vec![types::permissions::Scope::Company];
    if let Ok(agents) = store.list_agents(10_000, 0) {
        scopes.extend(agents.into_iter().map(|a| types::permissions::Scope::Employee(a.id)));
    }
    for scope in &scopes {
        for rule in store.permission_rules_in(scope).unwrap_or_default() {
            if let Some(types::permissions::RuleField::Folder(p)) = rule.field {
                folders.insert(p);
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(data_dir.join("worktrees")) {
        folders.extend(entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
    }
    folders.into_iter().collect()
}

/// Each project's `.nebo/hooks.yaml`, once.
fn hooks(folders: &[PathBuf], report: &mut Report) {
    let files: BTreeSet<PathBuf> = folders.iter().filter_map(|f| agent::shell_hooks::find_hooks_file(f)).collect();
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let at = path.display().to_string();
        let Some(moved) = rewrite_hooks(&text, &at, report) else { continue };
        if let Err(e) = std::fs::write(&path, moved) {
            warn!(path = %at, error = %e, "stored tool names: hooks file not rewritten");
        }
    }
}

/// A hooks file with each hook's filters on the current tool names, or
/// `None` when nothing in it names an old tool. A hook whose old tool keeps
/// some of its jobs splits in two: the moved jobs by name, and the kept tool
/// with its `resource` / `action` filters.
fn rewrite_hooks(text: &str, at: &str, report: &mut Report) -> Option<String> {
    let mut doc: serde_yaml::Value = serde_yaml::from_str(text).ok()?;
    let mut changed = false;
    for phase in ["pre_tool", "post_tool"] {
        let Some(serde_yaml::Value::Sequence(list)) = doc.get_mut(phase) else { continue };
        let mut out = Vec::with_capacity(list.len());
        for hook in list.drain(..) {
            match move_hook(&hook) {
                Some(moved) => {
                    changed = true;
                    let before = serde_yaml::to_string(&hook).unwrap_or_default();
                    let after = moved.iter().map(|h| serde_yaml::to_string(h).unwrap_or_default()).collect::<Vec<_>>();
                    report.changed(at, before.trim(), after.join("---\n").trim());
                    out.extend(moved);
                }
                None => out.push(hook),
            }
        }
        *list = out;
    }
    changed.then(|| serde_yaml::to_string(&doc).unwrap_or_default())
}

/// One hook on the current names, or `None` when it names no old tool.
fn move_hook(hook: &serde_yaml::Value) -> Option<Vec<serde_yaml::Value>> {
    let one_or_many = |key: &str| -> Vec<String> {
        match hook.get(key) {
            Some(serde_yaml::Value::String(s)) => vec![s.clone()],
            Some(serde_yaml::Value::Sequence(items)) => items.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
            _ => Vec::new(),
        }
    };
    let (tools, resources, actions) = (one_or_many("tool"), one_or_many("resource"), one_or_many("action"));
    // Filters with no tool matched the one tool that took them: os.
    let tools = if tools.is_empty() && !(resources.is_empty() && actions.is_empty()) { vec!["os".to_string()] } else { tools };
    let old = rename_map::old_tools();
    if !tools.iter().any(|t| old.contains(&t.as_str())) {
        return None;
    }
    let mut moved: Vec<String> = Vec::new();
    let mut kept: Vec<String> = Vec::new();
    for tool in &tools {
        if !old.contains(&tool.as_str()) {
            push_unique(&mut kept, tool.clone());
            continue;
        }
        let jobs = rename_map::RENAMES.iter().filter(|r| {
            r.tool == tool.as_str()
                && (resources.is_empty() || r.resource.is_none_or(|x| resources.iter().any(|v| v == x)))
                && (actions.is_empty() || r.action.is_none_or(|x| actions.iter().any(|v| v == x)))
        });
        for r in jobs.filter(|r| !r.to.contains('{')) {
            push_unique(&mut moved, r.to.to_string());
        }
        // The kept tool still answers the filters no row moved.
        let unmoved_resource = resources.is_empty()
            || resources.iter().any(|res| {
                !rename_map::RENAMES.iter().any(|r| r.tool == tool.as_str() && r.resource == Some(res.as_str()))
            });
        if rename_map::KEPT.contains(&tool.as_str()) && unmoved_resource {
            push_unique(&mut kept, tool.clone());
        }
    }
    if moved.is_empty() && kept == tools {
        return None;
    }
    let name = hook.get("name").and_then(|n| n.as_str()).unwrap_or("hook").to_string();
    let with = |tools: &[String], filtered: bool, suffix: &str| {
        let mut h = hook.clone();
        let map = h.as_mapping_mut().expect("a hook is a mapping");
        map.insert("tool".into(), serde_yaml::Value::Sequence(tools.iter().cloned().map(Into::into).collect()));
        if !filtered {
            map.remove("resource");
            map.remove("action");
        }
        map.insert("name".into(), format!("{name}{suffix}").into());
        h
    };
    let mut out = Vec::new();
    match (moved.is_empty(), kept.is_empty()) {
        (false, true) => out.push(with(&moved, false, "")),
        (true, false) => out.push(with(&kept, true, "")),
        (false, false) => {
            out.push(with(&moved, false, ""));
            out.push(with(&kept, true, "-os"));
        }
        (true, true) => return Some(Vec::new()),
    }
    Some(out)
}

/// Convert the workflow runs parked on the old approval card into asks,
/// once. Each parked call moves onto the current tool set and goes through
/// the one check as the run's own call: an ask is parked on the owner and
/// the run waits on it, as a run parked today does. When the owner's rules
/// now settle the call, the run is released with that answer.
///
/// The old card (`wf-approval:<run>`) is marked read here, and `resolve_card`
/// clears the hub's copy, whose Approve button no longer reaches a route.
/// The conversion is recorded only once every card is cleared: a hub that
/// can't be reached leaves it to the next start, which clears the cards of
/// every run already waiting on an ask and parks nothing twice.
pub(crate) async fn convert_parked_approvals<R, F>(
    store: &db::Store,
    registry: &tools::Registry,
    gate: &dyn tools::PermissionGate,
    resolve_card: R,
) -> Result<(), NeboError>
where
    R: Fn(String) -> F,
    F: std::future::Future<Output = Result<(), String>>,
{
    if store.upgrade_conversion_done(PARKED_APPROVALS)? {
        return Ok(());
    }
    let linked: BTreeSet<String> = store
        .open_permission_asks(None)?
        .into_iter()
        .filter_map(|a| a.run_id)
        .collect();
    let mut converted = Vec::new();
    let mut runs: BTreeSet<String> = linked.clone();
    for (run_id, agent_id, _binding, display, _) in store.list_workflow_suspensions()? {
        if linked.contains(&run_id) {
            continue;
        }
        let outcome = park_again(store, registry, gate, &run_id, &agent_id).await;
        info!(run_id = %run_id, outcome = %outcome, "parked approval converted");
        converted.push(serde_json::json!({ "run_id": run_id, "display": display, "outcome": outcome }));
        runs.insert(run_id);
    }
    let user_id = store.ensure_local_user_id().unwrap_or_default();
    let mut uncleared = 0;
    for run_id in &runs {
        let old_card = format!("wf-approval:{run_id}");
        if let Err(e) = store.mark_notification_read(&old_card, &user_id) {
            warn!(run_id = %run_id, error = %e, "old approval card not marked read");
        }
        if let Err(e) = resolve_card(old_card).await {
            warn!(run_id = %run_id, error = %e, "the hub's old approval card not cleared; tried again at the next start");
            uncleared += 1;
        }
    }
    if uncleared > 0 {
        return Ok(());
    }
    store.record_upgrade_conversion(PARKED_APPROVALS, &Value::Array(converted).to_string())
}

/// Move one parked run's call and put it through the check. Returns what
/// became of it.
async fn park_again(
    store: &db::Store,
    registry: &tools::Registry,
    gate: &dyn tools::PermissionGate,
    run_id: &str,
    agent_id: &str,
) -> String {
    let Ok(Some(parked)) = store.get_workflow_suspension(run_id) else {
        return "unreadable".to_string();
    };
    let pending = parked.6;
    let Ok(mut call) = serde_json::from_str::<ai::ToolCall>(&pending) else {
        return release(store, run_id, false, "its parked call is unreadable");
    };
    if let Some((name, input)) = rename_map::move_stored_call(&call.name, &call.input) {
        call.name = name;
        call.input = input;
        let moved = serde_json::to_string(&call).unwrap_or_default();
        if let Err(e) = store.set_workflow_suspension_call(run_id, &moved) {
            warn!(run_id, error = %e, "parked call not moved");
        }
    }
    let Some(tool) = registry.get(&call.name).await else {
        return release(store, run_id, false, &format!("no tool {} exists now", call.name));
    };
    let mut ctx = tools::ToolContext::new(tools::Origin::Workflow)
        .with_session(format!("agent:{agent_id}:workflow:{run_id}"), String::new());
    ctx.door = types::permissions::Door::Workflow;
    ctx.run_id = Some(run_id.to_string());
    let resolved = tools::ResolvedCall {
        tool: tool.as_ref(),
        input: &call.input,
        target: tools::registry::target_of(tool.as_ref(), &call.input),
    };
    match gate.check(&ctx, &resolved).await {
        tools::GateVerdict::Parked(result) => match result.parked_ask {
            Some(ask_id) => match store.link_permission_ask_run(&ask_id, run_id) {
                Ok(()) => format!("ask {ask_id}"),
                Err(e) => release(store, run_id, false, &format!("the ask could not name the run: {e}")),
            },
            None => release(store, run_id, false, "the ask was not written"),
        },
        tools::GateVerdict::Run(_) => release(store, run_id, true, "the owner's rules allow it"),
        tools::GateVerdict::Refuse(result) => release(store, run_id, false, &result.content),
    }
}

fn release(store: &db::Store, run_id: &str, allowed: bool, why: &str) -> String {
    if let Err(e) = store.engine_answer_wait(run_id, allowed) {
        warn!(run_id, error = %e, "parked run not released");
    }
    format!("released ({}): {why}", if allowed { "allowed" } else { "refused" })
}
