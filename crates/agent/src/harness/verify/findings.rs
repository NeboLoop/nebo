//! What a reviewer reports, and when the loop has stopped making progress.
//!
//! A reviewer's final answer is data (`turn_end::AnswerShapeCheck` holds it
//! to [`answer_schema`]): ranked findings, each with its evidence, a repro
//! and a fix; the checks it made that held; and, from the second round on,
//! what became of each earlier finding. A finding that is the owner's call
//! (a product or business decision, not a defect) says so, with options and
//! a recommended one: the loop asks the owner, never the builder.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Blocker,
    Major,
    Minor,
}

impl Severity {
    /// Blockers and majors keep the work from being verified.
    pub fn blocks(self) -> bool {
        matches!(self, Severity::Blocker | Severity::Major)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Something wrong with the code.
    #[default]
    Defect,
    /// A product or business call the owner makes.
    Decision,
}

/// One finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// Stable within the verification: `r<round>-<lens>-<n>`, given by the loop.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub lens: String,
    pub severity: Severity,
    #[serde(default)]
    pub kind: Kind,
    pub title: String,
    /// `path:line`.
    pub evidence: String,
    /// The steps or worked numbers that show it.
    pub repro: String,
    pub impact: String,
    pub fix: String,
    /// A decision's choices, and the one recommended.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended: Option<String>,
}

impl Finding {
    /// The same finding, whatever round reports it: its file and its title's
    /// words. A finding that comes back is the loop going round in circles.
    pub fn key(&self) -> String {
        let file = self
            .evidence
            .split(':')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let words: Vec<String> = self
            .title
            .to_ascii_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| w.len() > 2)
            .map(str::to_string)
            .collect();
        format!("{file}|{}", words.join(" "))
    }
}

/// A check a reviewer made that held.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Held {
    #[serde(default)]
    pub lens: String,
    pub checked: String,
}

/// What became of an earlier finding, as a later round's reviewer judges it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriorState {
    Fixed,
    Open,
    /// A minor the builder deferred with a reason the reviewer accepts.
    Deferred,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prior {
    pub id: String,
    pub state: PriorState,
    #[serde(default)]
    pub note: String,
}

/// One reviewer's answer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    #[serde(default)]
    pub findings: Vec<Finding>,
    #[serde(default)]
    pub verified_ok: Vec<Held>,
    #[serde(default)]
    pub prior: Vec<Prior>,
}

/// The JSON schema a reviewer's final answer must match.
pub fn answer_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["findings", "verified_ok"],
        "properties": {
            "findings": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["severity", "title", "evidence", "repro", "impact", "fix"],
                    "properties": {
                        "severity": { "type": "string", "enum": ["blocker", "major", "minor"] },
                        "kind": { "type": "string", "enum": ["defect", "decision"] },
                        "title": { "type": "string" },
                        "evidence": { "type": "string" },
                        "repro": { "type": "string" },
                        "impact": { "type": "string" },
                        "fix": { "type": "string" },
                        "options": { "type": "array", "items": { "type": "string" } },
                        "recommended": { "type": "string" }
                    }
                }
            },
            "verified_ok": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["checked"],
                    "properties": { "checked": { "type": "string" } }
                }
            },
            "prior": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["id", "state"],
                    "properties": {
                        "id": { "type": "string" },
                        "state": { "type": "string", "enum": ["fixed", "open", "deferred"] },
                        "note": { "type": "string" }
                    }
                }
            }
        }
    })
}

/// The findings still open after a round: the round's new findings, plus
/// every earlier one its reviewers did not judge fixed or deferred.
pub fn open_after(
    earlier_open: &[Finding],
    review_prior: &[Prior],
    new: &[Finding],
) -> Vec<Finding> {
    let closed: BTreeSet<&str> = review_prior
        .iter()
        .filter(|p| matches!(p.state, PriorState::Fixed | PriorState::Deferred))
        .map(|p| p.id.as_str())
        .collect();
    let mut open: Vec<Finding> = earlier_open
        .iter()
        .filter(|f| !closed.contains(f.id.as_str()))
        .cloned()
        .collect();
    for f in new {
        if !open.iter().any(|o| o.key() == f.key()) {
            open.push(f.clone());
        }
    }
    open
}

/// How many open findings block (blockers and majors).
pub fn blocking(open: &[Finding]) -> usize {
    open.iter().filter(|f| f.severity.blocks()).count()
}

/// Whether the loop has stopped making progress and the owner is asked:
/// two rounds in a row that did not lower the count of open blockers and
/// majors (`counts`, one per round, oldest first), or a blocking finding
/// that was fixed and came back.
pub fn stalled(counts: &[usize], came_back: bool) -> bool {
    if came_back {
        return true;
    }
    match counts {
        [.., a, b, c] => b >= a && c >= b,
        _ => false,
    }
}

/// Whether one of `new`'s blocking findings is one a reviewer judged fixed
/// in an earlier round (`fixed`: those findings).
pub fn came_back(fixed: &[Finding], new: &[Finding]) -> bool {
    let keys: BTreeSet<String> = fixed
        .iter()
        .filter(|f| f.severity.blocks())
        .map(Finding::key)
        .collect();
    new.iter()
        .any(|f| f.severity.blocks() && keys.contains(&f.key()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(id: &str, severity: Severity, evidence: &str, title: &str) -> Finding {
        Finding {
            id: id.into(),
            lens: "correctness".into(),
            severity,
            kind: Kind::Defect,
            title: title.into(),
            evidence: evidence.into(),
            repro: "r".into(),
            impact: "i".into(),
            fix: "f".into(),
            options: vec![],
            recommended: None,
        }
    }

    #[test]
    fn a_reviewers_answer_matches_its_schema() {
        let answer = serde_json::json!({
            "findings": [{"severity": "major", "title": "t", "evidence": "a.rs:3", "repro": "r", "impact": "i", "fix": "f"}],
            "verified_ok": [{"checked": "the migration runs twice"}]
        });
        assert!(a2ui_validation::validate(&answer_schema(), &answer, "answer").is_ok());
        let review: Review = serde_json::from_value(answer).unwrap();
        assert_eq!(review.findings[0].kind, Kind::Defect);
        let bad = serde_json::json!({"findings": [{"severity": "huge"}], "verified_ok": []});
        assert!(a2ui_validation::validate(&answer_schema(), &bad, "answer").is_err());
    }

    #[test]
    fn fixed_and_deferred_findings_close_and_the_rest_stay_open() {
        let earlier = vec![
            finding("r1-a", Severity::Blocker, "a.rs:1", "overflow"),
            finding("r1-b", Severity::Minor, "b.rs:1", "naming"),
            finding("r1-c", Severity::Major, "c.rs:1", "race"),
        ];
        let prior = vec![
            Prior {
                id: "r1-a".into(),
                state: PriorState::Fixed,
                note: String::new(),
            },
            Prior {
                id: "r1-b".into(),
                state: PriorState::Deferred,
                note: "cosmetic".into(),
            },
        ];
        let new = vec![finding("r2-x", Severity::Major, "d.rs:9", "leak")];
        let open = open_after(&earlier, &prior, &new);
        let ids: Vec<&str> = open.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["r1-c", "r2-x"]);
        assert_eq!(blocking(&open), 2);
    }

    #[test]
    fn two_rounds_without_progress_or_a_returning_finding_stall_the_loop() {
        assert!(!stalled(&[3, 2, 1], false));
        assert!(
            !stalled(&[3, 3], false),
            "one round without progress is not yet a stall"
        );
        assert!(stalled(&[3, 3, 3], false));
        assert!(stalled(&[2, 3, 4], false));
        assert!(!stalled(&[4, 4, 3], false));
        let fixed = vec![finding(
            "r1-a",
            Severity::Blocker,
            "src/a.rs:10",
            "Overflow in total",
        )];
        assert!(came_back(
            &fixed,
            &[finding(
                "r3-z",
                Severity::Major,
                "src/a.rs:12",
                "overflow in total"
            )]
        ));
        assert!(stalled(&[3, 1], true));
    }
}
