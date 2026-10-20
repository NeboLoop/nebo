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
    /// Where it points: its file and the lines it names
    /// ([`parse_evidence`]).
    pub fn place(&self) -> Place {
        parse_evidence(&self.evidence)
    }

    /// Whether `other` is this finding again, however a later round words
    /// it: the same lens, the same file, overlapping lines (within
    /// [`LINE_SLACK`], as code moves under a fix) and the same words in its
    /// evidence, or most of the same words in its title.
    pub fn same_as(&self, other: &Finding) -> bool {
        if self.lens != other.lens {
            return false;
        }
        let (a, b) = (self.place(), other.place());
        if a.file != b.file {
            return false;
        }
        let near = match (a.lines, b.lines) {
            (Some((a1, a2)), Some((b1, b2))) => a1 <= b2 + LINE_SLACK && b1 <= a2 + LINE_SLACK,
            _ => true,
        };
        near && (fingerprint(&self.evidence) == fingerprint(&other.evidence)
            && !fingerprint(&self.evidence).is_empty()
            || overlap(&words(&self.title), &words(&other.title)) >= 0.5)
    }
}

/// How far a finding's lines may move between rounds and still be the same.
pub const LINE_SLACK: u32 = 10;

/// A finding's place: the file (as written, case kept, Windows drive
/// letters included) and its line or line range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub file: String,
    pub lines: Option<(u32, u32)>,
}

/// `path:12`, `path:12-30`, `path:12:5`, `C:\src\a.rs:12`, or a path alone.
pub fn parse_evidence(evidence: &str) -> Place {
    let text = evidence
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(|c| c == '`' || c == '(' || c == ')' || c == ',');
    let mut file = text;
    let mut lines = None;
    // Peel `:line[-end]` (and a `:col`) off the end; a drive letter's colon
    // is followed by a separator, never by digits alone.
    let mut parts: Vec<&str> = text.rsplitn(3, ':').collect();
    parts.reverse();
    let number = |s: &str| -> Option<(u32, u32)> {
        let (a, b) = s.split_once('-').unwrap_or((s, s));
        Some((a.parse().ok()?, b.parse().ok()?))
    };
    match parts.as_slice() {
        [path, line, col] if number(line).is_some() && col.parse::<u32>().is_ok() => {
            file = path;
            lines = number(line);
        }
        [.., line] if number(line).is_some() => {
            let cut = text.len() - line.len() - 1;
            file = &text[..cut];
            lines = number(line);
        }
        _ => {}
    }
    Place {
        file: file.replace('\\', "/"),
        lines,
    }
}

/// The words of `text`: lowercase, alphanumeric, longer than two letters.
fn words(text: &str) -> BTreeSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .map(str::to_string)
        .collect()
}

/// An evidence line's words, without its path and numbers.
fn fingerprint(evidence: &str) -> String {
    let rest: String = evidence
        .split_whitespace()
        .skip(1)
        .collect::<Vec<_>>()
        .join(" ");
    words(&rest)
        .into_iter()
        .filter(|w| !w.chars().all(|c| c.is_ascii_digit()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// How much of the smaller set the two share.
fn overlap(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    let small = a.len().min(b.len());
    if small == 0 {
        return 0.0;
    }
    a.intersection(b).count() as f64 / small as f64
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
/// every earlier one its reviewers did not judge fixed or deferred. A new
/// finding that is an open one again is not counted twice.
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
        if !open.iter().any(|o| o.same_as(f)) {
            open.push(f.clone());
        }
    }
    open
}

/// How many open findings block (blockers and majors).
pub fn blocking(open: &[Finding]) -> usize {
    open.iter().filter(|f| f.severity.blocks()).count()
}

/// Rounds in a row without a new low before the loop pauses for the owner.
pub const ROUNDS_WITHOUT_PROGRESS: usize = 2;

/// Whether the loop has stopped making progress and the owner is asked:
/// the open blockers and majors (`counts`, one per round, oldest first)
/// have not reached a new low in [`ROUNDS_WITHOUT_PROGRESS`] rounds (so an
/// oscillation 4, 2, 4, 2 is caught), or a blocking finding that was fixed
/// came back.
pub fn stalled(counts: &[usize], came_back: bool) -> bool {
    if came_back {
        return true;
    }
    if counts.len() <= ROUNDS_WITHOUT_PROGRESS {
        return false;
    }
    let (before, recent) = counts.split_at(counts.len() - ROUNDS_WITHOUT_PROGRESS);
    let best_before = before.iter().copied().min().unwrap_or(usize::MAX);
    recent.iter().all(|&c| c >= best_before) && recent.iter().any(|&c| c > 0)
}

/// Whether one of `new`'s blocking findings is one a reviewer judged fixed
/// in an earlier round (`fixed`: those findings), however it is worded now.
pub fn came_back(fixed: &[Finding], new: &[Finding]) -> bool {
    new.iter().any(|f| {
        f.severity.blocks()
            && fixed
                .iter()
                .any(|old| old.severity.blocks() && old.same_as(f))
    })
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
    fn evidence_is_read_with_its_lines_on_any_system() {
        assert_eq!(
            parse_evidence("src/a.rs:12"),
            Place {
                file: "src/a.rs".into(),
                lines: Some((12, 12))
            }
        );
        assert_eq!(
            parse_evidence("src/a.rs:12-30 the loop"),
            Place {
                file: "src/a.rs".into(),
                lines: Some((12, 30))
            }
        );
        assert_eq!(
            parse_evidence("src/a.rs:12:5"),
            Place {
                file: "src/a.rs".into(),
                lines: Some((12, 12))
            }
        );
        assert_eq!(
            parse_evidence("C:\\work\\a.rs:7"),
            Place {
                file: "C:/work/a.rs".into(),
                lines: Some((7, 7))
            }
        );
        assert_eq!(
            parse_evidence("C:\\work\\a.rs"),
            Place {
                file: "C:/work/a.rs".into(),
                lines: None
            }
        );
        assert_eq!(
            parse_evidence("`README.md`"),
            Place {
                file: "README.md".into(),
                lines: None
            }
        );
    }

    #[test]
    fn a_finding_reworded_on_moved_lines_is_the_same_finding() {
        let a = finding(
            "r1-a",
            Severity::Blocker,
            "src/total.rs:40 sum skips the last item",
            "Off-by-one in total",
        );
        let reworded = finding(
            "r3-z",
            Severity::Major,
            "src/total.rs:44 sum skips the last item",
            "The total misses one line item",
        );
        assert!(
            a.same_as(&reworded),
            "same lens, same file, near lines, same evidence"
        );
        let elsewhere = finding(
            "r3-y",
            Severity::Major,
            "src/total.rs:400 sum skips the last item",
            "Off-by-one in total",
        );
        assert!(
            !a.same_as(&elsewhere),
            "far away in the file is another finding"
        );
        let mut other_lens = reworded.clone();
        other_lens.lens = "security".into();
        assert!(!a.same_as(&other_lens));
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
