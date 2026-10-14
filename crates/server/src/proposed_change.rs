//! What a proposed change does, worked out from the change itself.
//!
//! An employee's proposal (a learned skill, or a tuning of one of its own
//! workflows) is stored as the whole text it would write, beside the text it
//! replaces (`PendingWrite::prior_content`). The approval used to show that
//! text raw. This module reads the two and says what is different, so the
//! owner does not have to rely only on the employee's own account of the
//! change (`PendingWrite::explanation`):
//!
//! - [`changes`]: the readable list, "What changes": steps added, removed,
//!   renamed or changed (old → new text), the schedule before and after;
//!   for a skill, its description and sections. Plain data the app words in
//!   the owner's language; a schedule goes as its cron plus the owner's time
//!   zone, for the app's one schedule describer.
//! - [`raw_change`]: the line diff of the whole old and new text (a
//!   workflow pretty-printed with its keys in a stable order), for the
//!   approval's "Show raw change" view.
//!
//! Both are computed when the approval is read, from the row as stored, so a
//! proposal filed before this existed is described the same way.

use db::models::PendingWrite;
use serde::Serialize;
use serde_json::Value;

/// One readable difference between what is there now and what is proposed.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Change {
    /// A step the proposal adds. `text` is what the step is told to do.
    #[serde(rename_all = "camelCase")]
    StepAdded { step: String, text: String },
    /// A step the proposal takes out, with what it did.
    #[serde(rename_all = "camelCase")]
    StepRemoved { step: String, text: String },
    /// A step that keeps its instructions under a new name.
    #[serde(rename_all = "camelCase")]
    StepRenamed { before: String, after: String },
    /// A step whose instructions change: old → new.
    #[serde(rename_all = "camelCase")]
    StepChanged {
        step: String,
        before: String,
        after: String,
    },
    /// The same steps in a new order (`steps`, as they run now).
    #[serde(rename_all = "camelCase")]
    StepsReordered { steps: Vec<String> },
    /// When the workflow runs, before and after (None: it had none, or the
    /// workflow is new). `timezone` is the owner's zone (IANA) the schedule
    /// runs on, when known.
    #[serde(rename_all = "camelCase")]
    ScheduleChanged {
        before: Option<When>,
        after: Option<When>,
        timezone: Option<String>,
    },
    /// The one-line summary of the workflow or skill: old → new.
    #[serde(rename_all = "camelCase")]
    DescriptionChanged { before: String, after: String },
    /// A part of a skill's instructions (its heading's words; "" for the
    /// opening before any heading) that the proposal adds.
    #[serde(rename_all = "camelCase")]
    SectionAdded { section: String, text: String },
    #[serde(rename_all = "camelCase")]
    SectionRemoved { section: String, text: String },
    #[serde(rename_all = "camelCase")]
    SectionChanged {
        section: String,
        before: String,
        after: String,
    },
    /// Any other part of a workflow's definition, by its name: old → new.
    #[serde(rename_all = "camelCase")]
    SettingChanged {
        setting: String,
        before: String,
        after: String,
    },
}

/// What starts a workflow: a schedule (`cron`, normalized), or another
/// kind of start (`kind` alone: manual, event, heartbeat, ...).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct When {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
}

/// The whole old and new text, line by line.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawChange {
    /// "json" for a workflow, "markdown" for a skill.
    pub language: &'static str,
    /// Whether there was an original to compare with.
    pub original: Original,
    pub lines: Vec<RawLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Original {
    /// Compared with what is there now.
    Present,
    /// Nothing is there now: the proposal is all new.
    New,
    /// Something is there now, but the proposal was filed without a copy of
    /// it: the lines are the whole proposal, uncompared.
    Unavailable,
}

/// One line of a [`RawChange`]: kept (" "), removed ("-") or added ("+"),
/// with its line number in the old text, the new text, or both.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawLine {
    pub op: &'static str,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_line: Option<usize>,
}

/// A proposal as its approval reads it (GET /agents/learnings/{id}): the
/// row as stored (status, gist, action, target, content, and the
/// employee's `explanation` when it carries one), the employee's
/// `agentName`, and what the change does: `changes` and `rawChange`.
pub fn approval(row: &PendingWrite, agent_name: &str, timezone: Option<&str>) -> Value {
    let mut v = serde_json::to_value(row).unwrap_or_default();
    if let Some(m) = v.as_object_mut() {
        m.insert("agentName".into(), Value::from(agent_name));
        m.insert(
            "changes".into(),
            serde_json::to_value(changes(row, timezone)).unwrap_or_default(),
        );
        m.insert(
            "rawChange".into(),
            serde_json::to_value(raw_change(row)).unwrap_or_default(),
        );
    }
    v
}

/// The readable differences of a proposal. `timezone` is the owner's zone.
pub fn changes(row: &PendingWrite, timezone: Option<&str>) -> Vec<Change> {
    let before = row
        .prior_content
        .as_deref()
        .filter(|s| !s.trim().is_empty());
    match row.kind.as_str() {
        "workflow" => {
            let parse = |s: Option<&str>| s.and_then(|s| serde_json::from_str::<Value>(s).ok());
            match parse(row.content.as_deref()) {
                Some(after) => workflow_changes(parse(before).as_ref(), &after, timezone),
                None => Vec::new(),
            }
        }
        // A skill: compared only when both sides are there. A new skill is
        // read whole, and a deleted one is named by the approval itself.
        _ => match (before, row.content.as_deref()) {
            (Some(b), Some(a)) => skill_changes(b, a),
            _ => Vec::new(),
        },
    }
}

/// The line diff of a proposal against what it replaces.
pub fn raw_change(row: &PendingWrite) -> RawChange {
    let workflow = row.kind == "workflow";
    let text = |s: Option<&str>| -> Option<String> {
        let s = s.filter(|s| !s.trim().is_empty())?;
        if workflow {
            // Pretty-printed with every object's keys sorted: the same
            // definition reads the same however it was written.
            serde_json::from_str::<Value>(s)
                .ok()
                .and_then(|v| serde_json::to_string_pretty(&sorted(&v)).ok())
                .or_else(|| Some(s.to_string()))
        } else {
            Some(s.to_string())
        }
    };
    let new = text(row.content.as_deref()).unwrap_or_default();
    let old = text(row.prior_content.as_deref());
    let original = match (&old, row.action.as_str()) {
        (Some(_), _) => Original::Present,
        (None, "create") => Original::New,
        (None, _) => Original::Unavailable,
    };
    RawChange {
        language: if workflow { "json" } else { "markdown" },
        original,
        lines: line_diff(old.as_deref().unwrap_or(""), &new),
    }
}

/// `v` with every object's keys in sorted order.
fn sorted(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .map(|k| (k.clone(), sorted(&m[k])))
                    .collect(),
            )
        }
        Value::Array(a) => Value::Array(a.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

fn line_diff(old: &str, new: &str) -> Vec<RawLine> {
    let diff = similar::TextDiff::from_lines(old, new);
    diff.iter_all_changes()
        .map(|c| RawLine {
            op: match c.tag() {
                similar::ChangeTag::Equal => " ",
                similar::ChangeTag::Delete => "-",
                similar::ChangeTag::Insert => "+",
            },
            text: c.value().trim_end_matches(['\n', '\r']).to_string(),
            old_line: c.old_index().map(|i| i + 1),
            new_line: c.new_index().map(|i| i + 1),
        })
        .collect()
}

// ── Workflows ─────────────────────────────────────────────────────────────

/// A step as the owner reads it: its name, what it is told to do, and the
/// rest of its definition (compared, so a change there is not missed).
struct Step {
    id: String,
    name: String,
    text: String,
    rest: Value,
}

fn steps_of(binding: Option<&Value>) -> Vec<Step> {
    binding
        .and_then(|b| b.get("activities"))
        .and_then(|a| a.as_array())
        .map(|acts| {
            acts.iter()
                .map(|a| {
                    let id = a
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let label = a
                        .get("label")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty());
                    let name = label
                        .map(str::to_string)
                        .unwrap_or_else(|| crate::workforce_reporter::humanize(&id));
                    let intent = a
                        .get("intent")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim();
                    let steps: Vec<&str> = a
                        .get("steps")
                        .and_then(|v| v.as_array())
                        .map(|s| {
                            s.iter()
                                .filter_map(|x| x.as_str())
                                .map(str::trim)
                                .filter(|x| !x.is_empty())
                                .collect()
                        })
                        .unwrap_or_default();
                    let text = std::iter::once(intent)
                        .filter(|s| !s.is_empty())
                        .chain(steps)
                        .collect::<Vec<_>>()
                        .join("\n");
                    let mut rest = a.clone();
                    if let Some(m) = rest.as_object_mut() {
                        for k in ["id", "label", "intent", "steps"] {
                            m.remove(k);
                        }
                    }
                    Step {
                        id,
                        name,
                        text,
                        rest,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn when_of(binding: Option<&Value>) -> Option<When> {
    let t = binding?.get("trigger")?;
    let kind = t
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("manual")
        .to_string();
    let cron = (kind == "schedule")
        .then(|| {
            t.get("cron")
                .or_else(|| t.get("schedule"))
                .and_then(|v| v.as_str())
        })
        .flatten()
        .map(tools::PersonaTool::normalize_cron);
    Some(When { kind, cron })
}

fn workflow_changes(before: Option<&Value>, after: &Value, timezone: Option<&str>) -> Vec<Change> {
    let mut out = Vec::new();

    // When it runs.
    let (when_before, when_after) = (when_of(before), when_of(Some(after)));
    if when_before != when_after {
        out.push(Change::ScheduleChanged {
            before: when_before,
            after: when_after,
            timezone: timezone.map(str::to_string),
        });
    }

    // What it is.
    let description = |b: Option<&Value>| {
        b.and_then(|b| b.get("description"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let (desc_before, desc_after) = (description(before), description(Some(after)));
    if before.is_some() && desc_before != desc_after {
        out.push(Change::DescriptionChanged {
            before: desc_before,
            after: desc_after,
        });
    }

    // Its steps: matched by id, then a removed and an added step with the
    // same instructions are one step renamed.
    let (old, new) = (steps_of(before), steps_of(Some(after)));
    let mut removed: Vec<&Step> = old
        .iter()
        .filter(|o| !new.iter().any(|n| n.id == o.id))
        .collect();
    let mut steps_out = Vec::new();
    for n in &new {
        match old.iter().find(|o| o.id == n.id) {
            Some(o) => {
                if o.text != n.text {
                    steps_out.push(Change::StepChanged {
                        step: n.name.clone(),
                        before: o.text.clone(),
                        after: n.text.clone(),
                    });
                } else if o.rest != n.rest {
                    steps_out.push(Change::StepChanged {
                        step: n.name.clone(),
                        before: settings_text(&o.rest, &n.rest),
                        after: settings_text(&n.rest, &o.rest),
                    });
                } else if o.name != n.name {
                    steps_out.push(Change::StepRenamed {
                        before: o.name.clone(),
                        after: n.name.clone(),
                    });
                }
            }
            None => match removed
                .iter()
                .position(|o| o.text == n.text && o.rest == n.rest && !n.text.is_empty())
            {
                Some(i) => {
                    let o = removed.remove(i);
                    steps_out.push(Change::StepRenamed {
                        before: o.name.clone(),
                        after: n.name.clone(),
                    });
                }
                None => steps_out.push(Change::StepAdded {
                    step: n.name.clone(),
                    text: n.text.clone(),
                }),
            },
        }
    }
    for o in removed {
        out.push(Change::StepRemoved {
            step: o.name.clone(),
            text: o.text.clone(),
        });
    }
    out.extend(steps_out);

    // The same steps, moved: said once, in the new order.
    let order = |steps: &[Step]| steps.iter().map(|s| s.id.clone()).collect::<Vec<_>>();
    let (old_ids, new_ids) = (order(&old), order(&new));
    let same_set = old_ids.len() == new_ids.len() && old_ids.iter().all(|id| new_ids.contains(id));
    if same_set && old_ids != new_ids {
        out.push(Change::StepsReordered {
            steps: new.iter().map(|s| s.name.clone()).collect(),
        });
    }

    // Anything else in the definition (inputs, what it hands on, its
    // wiring), by name.
    if let (Some(b), Some(a)) = (before.and_then(|b| b.as_object()), after.as_object()) {
        let mut keys: Vec<&String> = b.keys().chain(a.keys()).collect();
        keys.sort();
        keys.dedup();
        for k in keys {
            if matches!(k.as_str(), "trigger" | "description" | "activities") {
                continue;
            }
            let (vb, va) = (b.get(k), a.get(k));
            if vb != va {
                out.push(Change::SettingChanged {
                    setting: crate::workforce_reporter::humanize(k),
                    before: vb.map(value_text).unwrap_or_default(),
                    after: va.map(value_text).unwrap_or_default(),
                });
            }
        }
    }
    out
}

/// The parts of `this` that differ from `other`, one "name: value" a line.
fn settings_text(this: &Value, other: &Value) -> String {
    let empty = serde_json::Map::new();
    let (t, o) = (
        this.as_object().unwrap_or(&empty),
        other.as_object().unwrap_or(&empty),
    );
    let mut keys: Vec<&String> = t.keys().chain(o.keys()).collect();
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .filter(|k| t.get(*k) != o.get(*k))
        .map(|k| {
            format!(
                "{}: {}",
                crate::workforce_reporter::humanize(k),
                t.get(k).map(value_text).unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A value as words: a string as itself, a list one item a line, anything
/// else compact.
fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) if a.iter().all(|x| x.is_string()) => a
            .iter()
            .filter_map(|x| x.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

// ── Skills ────────────────────────────────────────────────────────────────

/// A SKILL.md as its description and its sections (heading words → body).
fn skill_parts(text: &str) -> (String, Vec<(String, String)>) {
    let (front, body) = match text.strip_prefix("---") {
        Some(rest) => match rest.find("\n---") {
            Some(end) => (
                &rest[..end],
                rest[end + 4..].trim_start_matches(['\n', '\r']),
            ),
            None => ("", text),
        },
        None => ("", text),
    };
    let description = front
        .lines()
        .find_map(|l| l.trim().strip_prefix("description:"))
        .map(|d| d.trim().trim_matches(['"', '\'']).to_string())
        .unwrap_or_default();
    let mut sections: Vec<(String, String)> = vec![(String::new(), String::new())];
    for line in body.lines() {
        let heading = line.trim_start();
        if heading.starts_with('#') {
            sections.push((
                heading.trim_start_matches('#').trim().to_string(),
                String::new(),
            ));
        } else if let Some((_, text)) = sections.last_mut() {
            text.push_str(line);
            text.push('\n');
        }
    }
    let sections = sections
        .into_iter()
        .map(|(h, t)| (h, t.trim().to_string()))
        .filter(|(h, t)| !(h.is_empty() && t.is_empty()))
        .collect();
    (description, sections)
}

fn skill_changes(before: &str, after: &str) -> Vec<Change> {
    let (desc_b, old) = skill_parts(before);
    let (desc_a, new) = skill_parts(after);
    let mut out = Vec::new();
    if desc_b != desc_a {
        out.push(Change::DescriptionChanged {
            before: desc_b,
            after: desc_a,
        });
    }
    for (h, t) in &old {
        if !new.iter().any(|(nh, _)| nh == h) {
            out.push(Change::SectionRemoved {
                section: h.clone(),
                text: t.clone(),
            });
        }
    }
    for (h, t) in &new {
        match old.iter().find(|(oh, _)| oh == h) {
            Some((_, ot)) if ot != t => out.push(Change::SectionChanged {
                section: h.clone(),
                before: ot.clone(),
                after: t.clone(),
            }),
            Some(_) => {}
            None => out.push(Change::SectionAdded {
                section: h.clone(),
                text: t.clone(),
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(kind: &str, action: &str, prior: Option<Value>, content: Value) -> PendingWrite {
        PendingWrite {
            id: "p".into(),
            agent_id: "a".into(),
            kind: kind.into(),
            action: action.into(),
            target: "pace-the-budget".into(),
            content: Some(content.to_string()),
            gist: "Tune workflow 'pace-the-budget': split gather-spend".into(),
            target_hash: String::new(),
            prior_content: prior.map(|p| p.to_string()),
            explanation: None,
            status: "pending".into(),
            created_at: 0,
            resolved_at: None,
            resolved_via: None,
            resolved_tz: None,
        }
    }

    /// The owner's pace-the-budget workflow as it ran, and the tune that
    /// splits its gather-spend step.
    fn pace_before() -> Value {
        json!({
            "trigger": {"type": "schedule", "cron": "0 8 * * 1-5"},
            "description": "Keep ad spend on pace for the month",
            "activities": [
                {"id": "gather-spend", "intent": "Pull every spend report and total the month so far"},
                {"id": "work-out-pace", "intent": "Compare spend to the monthly budget"},
                {"id": "adjust-budget", "intent": "Raise or lower today's budget to stay on pace"}
            ]
        })
    }

    fn pace_after() -> Value {
        json!({
            "trigger": {"type": "schedule", "cron": "0 8 * * 1-5"},
            "description": "Keep ad spend on pace for the month",
            "activities": [
                {"id": "find-sources", "intent": "Find the spend sources for this month"},
                {"id": "pull-numbers", "intent": "Pull only the spend numbers from those sources"},
                {"id": "work-out-pace", "intent": "Compare spend to the monthly budget"},
                {"id": "adjust-budget", "intent": "Raise or lower today's budget to stay on pace"}
            ]
        })
    }

    #[test]
    fn a_split_step_reads_as_one_removed_and_two_added() {
        let r = row("workflow", "update", Some(pace_before()), pace_after());
        assert_eq!(
            changes(&r, Some("America/Denver")),
            vec![
                Change::StepRemoved {
                    step: "Gather spend".into(),
                    text: "Pull every spend report and total the month so far".into()
                },
                Change::StepAdded {
                    step: "Find sources".into(),
                    text: "Find the spend sources for this month".into()
                },
                Change::StepAdded {
                    step: "Pull numbers".into(),
                    text: "Pull only the spend numbers from those sources".into()
                },
            ]
        );
    }

    #[test]
    fn a_step_with_new_instructions_reads_old_then_new() {
        let mut after = pace_before();
        after["activities"][1]["intent"] = json!("Compare spend to the budget, prorated by day");
        after["activities"][1]["steps"] = json!(["Skip weekends"]);
        let r = row("workflow", "update", Some(pace_before()), after);
        assert_eq!(
            changes(&r, None),
            vec![Change::StepChanged {
                step: "Work out pace".into(),
                before: "Compare spend to the monthly budget".into(),
                after: "Compare spend to the budget, prorated by day\nSkip weekends".into(),
            }]
        );
    }

    #[test]
    fn a_step_under_a_new_name_reads_as_renamed() {
        let mut after = pace_before();
        after["activities"][0]["id"] = json!("total-spend");
        let r = row("workflow", "update", Some(pace_before()), after);
        assert_eq!(
            changes(&r, None),
            vec![Change::StepRenamed {
                before: "Gather spend".into(),
                after: "Total spend".into()
            }]
        );
    }

    #[test]
    fn a_step_whose_settings_change_says_which() {
        let mut after = pace_before();
        after["activities"][0]["tools"] = json!(["sheets"]);
        let r = row("workflow", "update", Some(pace_before()), after);
        assert_eq!(
            changes(&r, None),
            vec![Change::StepChanged {
                step: "Gather spend".into(),
                before: "Tools: ".into(),
                after: "Tools: sheets".into()
            }]
        );
    }

    #[test]
    fn a_removed_step_and_reordered_steps() {
        let mut after = pace_before();
        after["activities"] = json!([
            pace_before()["activities"][2],
            pace_before()["activities"][1]
        ]);
        let r = row("workflow", "update", Some(pace_before()), after);
        assert_eq!(
            changes(&r, None),
            vec![Change::StepRemoved {
                step: "Gather spend".into(),
                text: "Pull every spend report and total the month so far".into()
            }]
        );
        let mut swapped = pace_before();
        swapped["activities"] = json!([
            pace_before()["activities"][1],
            pace_before()["activities"][0],
            pace_before()["activities"][2]
        ]);
        let r = row("workflow", "update", Some(pace_before()), swapped);
        assert_eq!(
            changes(&r, None),
            vec![Change::StepsReordered {
                steps: vec![
                    "Work out pace".into(),
                    "Gather spend".into(),
                    "Adjust budget".into()
                ]
            }]
        );
    }

    #[test]
    fn a_schedule_change_carries_both_crons_and_the_owners_zone() {
        let mut after = pace_before();
        after["trigger"] = json!({"type": "schedule", "cron": "30 7 * * 1-5"});
        let r = row("workflow", "update", Some(pace_before()), after);
        let got = changes(&r, Some("America/Denver"));
        assert_eq!(
            got,
            vec![Change::ScheduleChanged {
                before: Some(When {
                    kind: "schedule".into(),
                    cron: Some("0 0 8 * * MON-FRI *".into())
                }),
                after: Some(When {
                    kind: "schedule".into(),
                    cron: Some("0 30 7 * * MON-FRI *".into())
                }),
                timezone: Some("America/Denver".into()),
            }]
        );
        let wire = serde_json::to_value(&got[0]).unwrap();
        assert_eq!(wire["kind"], "scheduleChanged");
        assert_eq!(wire["before"]["type"], "schedule");
        assert_eq!(wire["after"]["cron"], "0 30 7 * * MON-FRI *");
        assert_eq!(wire["timezone"], "America/Denver");

        // The same schedule written another way is no change.
        let mut same = pace_before();
        same["trigger"] = json!({"type": "schedule", "cron": "0 0 8 * * MON-FRI *"});
        assert!(changes(&row("workflow", "update", Some(pace_before()), same), None).is_empty());
    }

    #[test]
    fn a_new_workflow_shows_its_schedule_and_every_step() {
        let r = row("workflow", "create", None, pace_after());
        let got = changes(&r, Some("America/Denver"));
        assert_eq!(
            got[0],
            Change::ScheduleChanged {
                before: None,
                after: Some(When {
                    kind: "schedule".into(),
                    cron: Some("0 0 8 * * MON-FRI *".into())
                }),
                timezone: Some("America/Denver".into()),
            }
        );
        let added: Vec<&str> = got[1..]
            .iter()
            .map(|c| match c {
                Change::StepAdded { step, .. } => step.as_str(),
                other => panic!("only steps added after the schedule: {other:?}"),
            })
            .collect();
        assert_eq!(
            added,
            [
                "Find sources",
                "Pull numbers",
                "Work out pace",
                "Adjust budget"
            ]
        );
        let raw = raw_change(&r);
        assert_eq!(raw.original, Original::New);
        assert!(raw.lines.iter().all(|l| l.op == "+"));
    }

    #[test]
    fn other_settings_are_named() {
        let mut after = pace_before();
        after["emit"] = json!("budget.adjusted");
        let r = row("workflow", "update", Some(pace_before()), after);
        assert_eq!(
            changes(&r, None),
            vec![Change::SettingChanged {
                setting: "Emit".into(),
                before: String::new(),
                after: "budget.adjusted".into()
            }]
        );
    }

    #[test]
    fn the_raw_change_is_a_line_diff_in_a_stable_key_order() {
        // The same definition with its keys in another order: no change.
        let reordered: Value = serde_json::from_str(
            r#"{"activities":[{"intent":"Pull every spend report and total the month so far","id":"gather-spend"},
                {"intent":"Compare spend to the monthly budget","id":"work-out-pace"},
                {"intent":"Raise or lower today's budget to stay on pace","id":"adjust-budget"}],
                "description":"Keep ad spend on pace for the month","trigger":{"cron":"0 8 * * 1-5","type":"schedule"}}"#,
        )
        .unwrap();
        let same = raw_change(&row("workflow", "update", Some(pace_before()), reordered));
        assert_eq!(same.original, Original::Present);
        assert!(same.lines.iter().all(|l| l.op == " "), "{:?}", same.lines);

        let raw = raw_change(&row(
            "workflow",
            "update",
            Some(pace_before()),
            pace_after(),
        ));
        assert_eq!(raw.language, "json");
        let removed: Vec<&str> = raw
            .lines
            .iter()
            .filter(|l| l.op == "-")
            .map(|l| l.text.trim())
            .collect();
        let added: Vec<&str> = raw
            .lines
            .iter()
            .filter(|l| l.op == "+")
            .map(|l| l.text.trim())
            .collect();
        assert!(
            removed.contains(&"\"id\": \"gather-spend\","),
            "{removed:?}"
        );
        assert!(added.contains(&"\"id\": \"find-sources\","), "{added:?}");
        assert!(added.contains(&"\"id\": \"pull-numbers\","), "{added:?}");
        // Kept lines carry both numbers, removed only the old, added only the new.
        let first = &raw.lines[0];
        assert_eq!(
            (first.op, first.old_line, first.new_line),
            (" ", Some(1), Some(1))
        );
        assert!(
            raw.lines
                .iter()
                .filter(|l| l.op == "-")
                .all(|l| l.old_line.is_some() && l.new_line.is_none())
        );
        assert!(
            raw.lines
                .iter()
                .filter(|l| l.op == "+")
                .all(|l| l.new_line.is_some() && l.old_line.is_none())
        );
    }

    #[test]
    fn an_update_filed_without_its_original_shows_the_proposal_whole() {
        let r = row("workflow", "update", None, pace_after());
        let raw = raw_change(&r);
        assert_eq!(raw.original, Original::Unavailable);
        assert!(raw.lines.iter().all(|l| l.op == "+"));
    }

    /// The pace-the-budget tune as its approval reads it, explained; and
    /// the same proposal filed before proposals carried an explanation,
    /// which keeps every field it had (the app falls back to its gist) and
    /// gains the server's own account of the change.
    #[test]
    fn the_approval_payload_with_and_without_an_explanation() {
        let mut r = row("workflow", "update", Some(pace_before()), pace_after());
        let old = approval(&r, "Ledger", Some("America/Denver"));
        assert!(old.get("explanation").is_none());
        for key in [
            "id",
            "kind",
            "action",
            "target",
            "content",
            "gist",
            "status",
            "priorContent",
            "agentName",
        ] {
            assert!(old.get(key).is_some(), "an old proposal keeps `{key}`");
        }
        assert_eq!(
            old["gist"],
            "Tune workflow 'pace-the-budget': split gather-spend"
        );
        assert_eq!(old["changes"].as_array().map(Vec::len), Some(3));
        assert_eq!(old["rawChange"]["original"], "present");

        r.explanation = Some(db::models::ChangeExplanation {
            what_went_wrong: "Last Tuesday's run stopped halfway: the gather-spend step ran out of turns pulling every report.".into(),
            the_fix: "Split gather-spend into two steps: find the sources, then pull only the numbers.".into(),
            what_happens_now: "Weekdays at 8:00 AM it finds the spend sources, pulls the numbers, works out the pace and adjusts the budget. No step runs out of turns.".into(),
        });
        let v = approval(&r, "Ledger", Some("America/Denver"));
        assert_eq!(
            v["explanation"]["theFix"],
            "Split gather-spend into two steps: find the sources, then pull only the numbers."
        );
        assert_eq!(
            v["changes"][0],
            json!({"kind": "stepRemoved", "step": "Gather spend", "text": "Pull every spend report and total the month so far"})
        );
        assert_eq!(
            v["changes"][1],
            json!({"kind": "stepAdded", "step": "Find sources", "text": "Find the spend sources for this month"})
        );
    }

    #[test]
    fn a_skill_update_reads_as_description_and_sections() {
        let before = "---\nname: invoices\ndescription: Chase invoices\n---\nOpening.\n## Steps\nList every invoice.\n## Tone\nFriendly.\n";
        let after = "---\nname: invoices\ndescription: Chase overdue invoices\n---\nOpening.\n## Steps\nList only overdue invoices.\n## Sign-off\nAlways sign as the owner.\n";
        let mut r = row("skill", "update", None, json!(null));
        r.prior_content = Some(before.into());
        r.content = Some(after.into());
        assert_eq!(
            changes(&r, None),
            vec![
                Change::DescriptionChanged {
                    before: "Chase invoices".into(),
                    after: "Chase overdue invoices".into()
                },
                Change::SectionRemoved {
                    section: "Tone".into(),
                    text: "Friendly.".into()
                },
                Change::SectionChanged {
                    section: "Steps".into(),
                    before: "List every invoice.".into(),
                    after: "List only overdue invoices.".into()
                },
                Change::SectionAdded {
                    section: "Sign-off".into(),
                    text: "Always sign as the owner.".into()
                },
            ]
        );
        let raw = raw_change(&r);
        assert_eq!(
            (raw.language, raw.original),
            ("markdown", Original::Present)
        );
        assert!(
            raw.lines
                .iter()
                .any(|l| l.op == "-" && l.text == "List every invoice.")
        );
        assert!(
            raw.lines
                .iter()
                .any(|l| l.op == "+" && l.text == "List only overdue invoices.")
        );
    }
}
