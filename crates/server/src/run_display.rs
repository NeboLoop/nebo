//! Human-readable projections of workflow runs.
//!
//! Every activity type emits its own output shape (a condition emits a
//! verdict, a command emits whatever it printed, an agent emits prose, a
//! watch trigger emits a raw provider payload). The UI must never have to
//! know those shapes — the engine narrates. These are pure functions applied
//! at READ time when a run is served: deterministic projections of the
//! recorded data (runs are the audit trail — no second copy that can drift,
//! no model in the path), and historical runs become readable with no
//! migration.
//!
//! Facts use machine keys (`from`, `subject`, …) so the client can localize
//! known ones; unknown keys render as-is — they are data, not chrome.

use serde_json::{Value, json};

/// The full display projection for one run: the input summary plus every
/// activity's narration. The activity definitions supply type/intent — agent
/// runs keep theirs on the binding, legacy workflows in the definition JSON.
/// ONE builder: the run-detail endpoint and the work tool's chat receipt both
/// come through here.
pub fn for_run(store: &db::Store, run: &db::models::WorkflowRun) -> Value {
    let activity_defs: Option<Value> =
        types::keyparser::agent_id_from_workflow_id(&run.workflow_id)
            .and_then(|agent_id| {
                let binding = run
                    .trigger_detail
                    .as_deref()
                    .map(|d| d.split(':').next().unwrap_or(d))?;
                store
                    .list_agent_workflows(agent_id)
                    .ok()?
                    .into_iter()
                    .find(|w| w.binding_name == binding)
                    .and_then(|w| w.activities)
            })
            .or_else(|| {
                let wf = store.get_workflow(&run.workflow_id).ok().flatten()?;
                serde_json::from_str::<Value>(&wf.definition)
                    .ok()?
                    .get("activities")
                    .cloned()
            });
    json!({
        "input": input_display(run.inputs.as_deref()),
        "activities": activities_display(run.output.as_deref(), activity_defs.as_ref()),
    })
}

const LINE_MAX: usize = 140;
const VALUE_MAX: usize = 120;
const FACTS_MAX: usize = 10;

/// Summarize a run's inputs. `Value::Null` when there is nothing worth
/// saying (empty/manual inputs) — the client falls back to its raw view.
pub fn input_display(inputs: Option<&str>) -> Value {
    let Some(raw) = inputs else { return Value::Null };
    let Ok(parsed) = serde_json::from_str::<Value>(raw) else {
        return Value::Null;
    };
    if let Some(watch) = parsed.get("_watch_payload") {
        let mut watch = watch.clone();
        agent::agent_worker::normalize_watch_payload(&mut watch);
        let mut facts: Vec<Value> = Vec::new();
        for key in ["from", "subject", "date"] {
            if let Some(v) = watch.get(key).and_then(|v| v.as_str())
                && !v.is_empty()
            {
                facts.push(json!({"key": key, "value": clip(v, VALUE_MAX)}));
            }
        }
        for name in attachment_names(&watch) {
            facts.push(json!({"key": "attachment", "value": name}));
        }
        if let Some(src) = parsed.get("_watch_source").and_then(|v| v.as_str()) {
            let event = watch.get("event").and_then(|v| v.as_str()).unwrap_or("");
            let via = if event.is_empty() {
                src.to_string()
            } else {
                format!("{src} · {event}")
            };
            facts.push(json!({"key": "via", "value": via}));
        }
        let line = watch
            .get("subject")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| clip(s, LINE_MAX));
        if line.is_none() && facts.is_empty() {
            return Value::Null;
        }
        return json!({"line": line, "facts": facts});
    }
    // Generic inputs: surface top-level scalars, skip reserved keys.
    let facts = scalar_facts(&parsed, "");
    if facts.is_empty() {
        return Value::Null;
    }
    json!({"line": Value::Null, "facts": facts})
}

/// Summarize every activity of a run: `{activity_id: {line, verdict?, facts}}`.
///
/// `output` is the run's concatenated output blob (`[Activity 'x' result]: …`
/// sections); `defs` the workflow definition's activities (for type/intent).
pub fn activities_display(output: Option<&str>, defs: Option<&Value>) -> Value {
    let outputs = split_outputs(output.unwrap_or(""));
    let empty = vec![];
    let defs: &Vec<Value> = defs.and_then(|d| d.as_array()).unwrap_or(&empty);
    let mut map = serde_json::Map::new();
    let ids: Vec<String> = defs
        .iter()
        .filter_map(|d| d.get("id").and_then(|i| i.as_str()).map(str::to_string))
        .chain(outputs.keys().cloned())
        .collect();
    for id in ids {
        if map.contains_key(&id) {
            continue;
        }
        let def = defs
            .iter()
            .find(|d| d.get("id").and_then(|i| i.as_str()) == Some(id.as_str()));
        map.insert(
            id.clone(),
            one_activity(def, outputs.get(&id).map(String::as_str)),
        );
    }
    Value::Object(map)
}

fn one_activity(def: Option<&Value>, output: Option<&str>) -> Value {
    let kind = def
        .and_then(|d| d.get("type"))
        .and_then(|t| t.as_str())
        .unwrap_or("");
    let intent = def
        .and_then(|d| d.get("intent"))
        .and_then(|i| i.as_str())
        .unwrap_or("");
    let out = output.unwrap_or("").trim();

    match kind {
        "condition" => {
            // Output is the verdict; the intent says what was asked.
            let verdict = match out {
                "True" => "passed",
                "False" => "stopped",
                _ => "",
            };
            let line = if intent.is_empty() {
                def.and_then(|d| d.get("params"))
                    .and_then(|p| p.get("expression"))
                    .and_then(|e| e.as_str())
                    .unwrap_or("")
            } else {
                intent
            };
            json!({"line": clip(line, LINE_MAX), "verdict": verdict, "facts": []})
        }
        "loop" => {
            // The engine already narrates loops ("5 items processed").
            let line = if out.is_empty() { intent } else { out };
            json!({"line": clip(line, LINE_MAX), "facts": []})
        }
        "command" => {
            // JSON output becomes facts; the intent stays the headline.
            let facts = serde_json::from_str::<Value>(out)
                .map(|v| scalar_facts(&v, ""))
                .unwrap_or_default();
            let line = if !intent.is_empty() {
                intent.to_string()
            } else {
                first_line(out)
            };
            json!({"line": clip(&line, LINE_MAX), "facts": facts})
        }
        // Agent activities (and unknown types): output is prose — lead with
        // it; JSON-shaped output falls back to facts like a command.
        _ => {
            if let Ok(v) = serde_json::from_str::<Value>(out) {
                let facts = scalar_facts(&v, "");
                if !facts.is_empty() {
                    return json!({"line": clip(intent, LINE_MAX), "facts": facts});
                }
            }
            let line = if out.is_empty() {
                intent.to_string()
            } else {
                first_line(out)
            };
            json!({"line": clip(&line, LINE_MAX), "facts": []})
        }
    }
}

/// Split the run's concatenated output blob into per-activity sections.
fn split_outputs(blob: &str) -> std::collections::BTreeMap<String, String> {
    let mut map = std::collections::BTreeMap::new();
    let marker = "[Activity '";
    let mut sections: Vec<(String, usize)> = Vec::new();
    let mut pos = 0;
    while let Some(found) = blob[pos..].find(marker) {
        let start = pos + found + marker.len();
        if let Some(end_quote) = blob[start..].find("' result]:") {
            let id = blob[start..start + end_quote].to_string();
            sections.push((id, start + end_quote + "' result]:".len()));
            pos = start + end_quote;
        } else {
            break;
        }
    }
    for i in 0..sections.len() {
        let (id, body_start) = &sections[i];
        let body_end = if i + 1 < sections.len() {
            blob[..sections[i + 1].1].rfind(marker).unwrap_or(blob.len())
        } else {
            blob.len()
        };
        map.insert(
            id.clone(),
            blob[*body_start..body_end.max(*body_start)].trim().to_string(),
        );
    }
    map
}

/// Flatten a JSON value into display facts: top-level scalars, array lengths,
/// and one level into small all-scalar objects (`counts.committed`). Reserved
/// (`_`-prefixed) keys are skipped; capped so a wide payload can't flood the UI.
fn scalar_facts(v: &Value, prefix: &str) -> Vec<Value> {
    let mut facts = Vec::new();
    let Some(obj) = v.as_object() else {
        return facts;
    };
    for (k, val) in obj {
        if facts.len() >= FACTS_MAX {
            break;
        }
        if k.starts_with('_') {
            continue;
        }
        let key = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        match val {
            Value::String(s) if !s.is_empty() => {
                facts.push(json!({"key": key, "value": clip(s, VALUE_MAX)}));
            }
            Value::Number(n) => facts.push(json!({"key": key, "value": n.to_string()})),
            Value::Bool(b) => facts.push(json!({"key": key, "value": b.to_string()})),
            Value::Array(a) if !a.is_empty() => {
                let all_short_strings = a.len() <= 3
                    && a.iter().all(|x| {
                        x.as_str().is_some_and(|s| s.len() <= 24)
                    });
                let value = if all_short_strings {
                    a.iter()
                        .filter_map(|x| x.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                } else {
                    format!("{} items", a.len())
                };
                facts.push(json!({"key": key, "value": value}));
            }
            Value::Object(inner)
                if prefix.is_empty()
                    && inner.len() <= 6
                    && inner.values().all(|x| !x.is_object() && !x.is_array()) =>
            {
                facts.extend(scalar_facts(val, k));
            }
            _ => {}
        }
    }
    facts.truncate(FACTS_MAX);
    facts
}

fn attachment_names(watch: &Value) -> Vec<String> {
    fn walk(part: &Value, out: &mut Vec<String>) {
        if let Some(name) = part.get("filename").and_then(|f| f.as_str())
            && !name.is_empty()
        {
            out.push(name.to_string());
        }
        if let Some(parts) = part.get("parts").and_then(|p| p.as_array()) {
            for p in parts {
                walk(p, out);
            }
        }
    }
    let mut out = Vec::new();
    if let Some(payload) = watch.get("payload") {
        walk(payload, &mut out);
    }
    out.truncate(3);
    out
}

fn first_line(s: &str) -> String {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string()
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    // The EXACT payload shape Gmail delivered on the dropped 4:37am report.
    fn raw_gmail_inputs() -> String {
        json!({
            "_watch_payload": {
                "event": "email.new",
                "id": "1a033574ee0ae0e2",
                "payload": {
                    "headers": [
                        {"name": "From", "value": "<WindowConfirmation@alside.com>"},
                        {"name": "Subject", "value": "Open Order Report: 87495 VIVID WINDOWS"},
                        {"name": "Date", "value": "Mon, 24 Aug 2026 03:35:55 -0700"},
                        {"name": "ARC-Seal", "value": "i=1; a=rsa-sha256; ..."}
                    ],
                    "parts": [
                        {"filename": "", "mimeType": "text/plain"},
                        {"filename": "Open Order Report.xls", "mimeType": "application/vnd.ms-excel"}
                    ]
                }
            },
            "_watch_source": "gmail"
        })
        .to_string()
    }

    #[test]
    fn watch_input_lifts_the_email_not_the_header_soup() {
        let d = input_display(Some(&raw_gmail_inputs()));
        assert_eq!(
            d["line"].as_str().unwrap(),
            "Open Order Report: 87495 VIVID WINDOWS"
        );
        let facts = d["facts"].as_array().unwrap();
        let get = |k: &str| {
            facts
                .iter()
                .find(|f| f["key"] == k)
                .map(|f| f["value"].as_str().unwrap().to_string())
        };
        assert_eq!(get("from").unwrap(), "<WindowConfirmation@alside.com>");
        assert_eq!(get("attachment").unwrap(), "Open Order Report.xls");
        assert_eq!(get("via").unwrap(), "gmail · email.new");
        // ARC seals and received chains never surface.
        assert!(facts.iter().all(|f| f["key"] != "ARC-Seal"));
    }

    #[test]
    fn empty_or_unparseable_inputs_display_nothing() {
        assert!(input_display(None).is_null());
        assert!(input_display(Some("{}")).is_null());
        assert!(input_display(Some("not json")).is_null());
    }

    #[test]
    fn condition_command_agent_and_loop_each_narrate() {
        let defs = json!([
            {"id": "gate", "type": "condition", "intent": "Only the factory"},
            {"id": "commit", "type": "command", "intent": "Commit the log"},
            {"id": "resolve", "type": "", "intent": "Resolve rows"},
            {"id": "chunks", "type": "loop", "intent": "Process each chunk"}
        ]);
        let blob = "[Activity 'gate' result]: True \
                    [Activity 'commit' result]: {\"committed\": [\"X1\",\"X2\"], \"counts\": {\"rows\": 83}} \
                    [Activity 'resolve' result]: Chunk 4 logged successfully.\nDetails follow. \
                    [Activity 'chunks' result]: 5 items processed";
        let d = activities_display(Some(blob), Some(&defs));
        assert_eq!(d["gate"]["verdict"], "passed");
        assert_eq!(d["gate"]["line"], "Only the factory");
        assert_eq!(d["commit"]["line"], "Commit the log");
        let commit_facts = d["commit"]["facts"].as_array().unwrap();
        assert!(commit_facts.iter().any(|f| f["key"] == "committed" && f["value"] == "X1, X2"));
        assert!(commit_facts.iter().any(|f| f["key"] == "counts.rows" && f["value"] == "83"));
        assert_eq!(d["resolve"]["line"], "Chunk 4 logged successfully.");
        assert_eq!(d["chunks"]["line"], "5 items processed");
    }

    #[test]
    fn condition_stopped_and_unknown_activities_survive() {
        let defs = json!([{"id": "gate", "type": "condition", "intent": "Only the factory"}]);
        let d = activities_display(Some("[Activity 'gate' result]: False"), Some(&defs));
        assert_eq!(d["gate"]["verdict"], "stopped");
        // Output for an activity with no definition still gets a line.
        let d = activities_display(Some("[Activity 'mystery' result]: it ran"), None);
        assert_eq!(d["mystery"]["line"], "it ran");
    }
}
