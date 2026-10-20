//! A review round: one independent reviewer per lens, in parallel, each in a
//! fresh context.
//!
//! A reviewer is a Review helper ([`delegation::HelperKind::Review`]) that
//! hears only its brief ([`brief`]): the request, the change, where the
//! code is, the round's check results and the rules. It never sees how the
//! builder worked. It is told to refute the change and to drop anything it
//! cannot show, and it answers as data ([`findings::answer_schema`]). It
//! runs on the employee's own model and level, under the same limits as any
//! of the employee's work, and the owner's Stop ends it.

use std::path::Path;
use std::sync::Arc;

use super::checks::CheckRun;
use super::findings::{self, Finding, Review};
use super::lenses::Lens;
use super::shadow::ChangeSet;
use crate::harness::delegation::{Ask, HelperKind, Helpers, child::Parent};

/// The most of a patch a brief carries inline; a longer one is read from
/// its file.
pub const INLINE_PATCH_CHARS: usize = 60_000;

/// What a round's reviewers are told.
pub struct RoundInput<'a> {
    pub verification_id: &'a str,
    pub round: u32,
    /// A later round: the reviewer checks the fixes (the delta) and the
    /// riskiest area, with the earlier findings as context.
    pub delta: bool,
    pub lenses: &'a [Lens],
    /// What was asked.
    pub spec: &'a str,
    /// What the round reviews: the whole change, or the round's delta.
    pub change: &'a ChangeSet,
    /// The whole change's stat, for a delta round's context.
    pub whole_stat: &'a str,
    /// Where the patch is saved whole.
    pub patch_file: &'a Path,
    /// The project with the change applied.
    pub work_dir: &'a Path,
    /// The reviewer's build folder.
    pub build_dir: &'a Path,
    /// The round's check results.
    pub checks: &'a [CheckRun],
    /// Findings still open from earlier rounds.
    pub prior_open: &'a [Finding],
    /// What the builder said about the findings (fixed, or deferred and why).
    pub builder_notes: &'a str,
    /// The rules section ([`super::rules::brief_rules`]).
    pub rules: &'a str,
}

/// One reviewer's result.
#[derive(Debug, Clone)]
pub struct LensReview {
    pub lens: Lens,
    /// The review, or why there is none (it stopped, failed, or never
    /// answered in shape).
    pub review: Result<Review, String>,
    /// Its session, where its steps and their cost are kept.
    pub session_key: String,
}

/// The node a reviewer's session is named by.
pub fn node(verification_id: &str, round: u32, lens: Lens) -> String {
    format!("verify-{verification_id}-r{round}-{}", lens.as_str())
}

/// Run one round: a reviewer per lens, all at once, for the turn `parent`.
pub async fn run_round(
    helpers: &Arc<Helpers>,
    parent: &Parent<'_>,
    input: &RoundInput<'_>,
) -> Vec<LensReview> {
    let asks = input.lenses.iter().map(|&lens| {
        let ask = Ask {
            node: node(input.verification_id, input.round, lens),
            description: format!("Reviewing: {}", lens.as_str()),
            kind: HelperKind::Review,
            prompt: brief(lens, input),
            schema: findings::answer_schema(),
            tools: None,
            fence: vec![
                input.work_dir.to_string_lossy().into_owned(),
                input.build_dir.to_string_lossy().into_owned(),
            ],
            shown: true,
        };
        async move {
            let answered = helpers.answer(parent, ask, None).await;
            let review = answered.value.and_then(|v| {
                serde_json::from_value::<Review>(v)
                    .map_err(|e| format!("its answer could not be read: {e}"))
            });
            LensReview {
                lens,
                review: review.map(|r| numbered(r, input.round, lens)),
                session_key: answered.session_key,
            }
        }
    });
    futures::future::join_all(asks).await
}

/// Give each finding its id and lens: `r<round>-<lens>-<n>`.
fn numbered(mut review: Review, round: u32, lens: Lens) -> Review {
    for (n, f) in review.findings.iter_mut().enumerate() {
        f.id = format!("r{round}-{}-{}", lens.as_str(), n + 1);
        f.lens = lens.as_str().to_string();
    }
    for held in &mut review.verified_ok {
        held.lens = lens.as_str().to_string();
    }
    review
}

/// A reviewer's whole brief: it sees nothing else.
pub fn brief(lens: Lens, input: &RoundInput<'_>) -> String {
    let mut out = String::new();
    out.push_str(
        "You are an independent reviewer of a code change. You did not write it and you have not seen how it was \
         written: judge only the request, the change and the code.\n\n",
    );
    out.push_str(&format!("## Your one lens\n{}\n\n", lens.charter()));
    out.push_str(
        "## Your job: refute it\nTry hard to show the change is wrong, unsafe or incomplete. Read the code around it, \
         follow its callers, run the build and the tests, and try the edge cases. Report only what you can show: every \
         finding needs evidence (path:line) and a concrete repro or worked numbers. A finding you cannot reproduce is \
         dropped, not reported. When nothing you tried breaks it, say what you checked in `verified_ok`.\n\n",
    );
    if input.delta {
        out.push_str(&format!(
            "## This is round {}\nEarlier rounds reviewed the whole change. This round reviews what changed since: the \
             fixes below, and the riskiest part of the whole change around them. Check every earlier finding and say in \
             `prior` whether it is fixed, still open, or deferred (a minor whose deferral reason you accept).\n\n",
            input.round
        ));
    }
    out.push_str(&format!("## The request\n{}\n\n", input.spec.trim()));
    out.push_str(&format!(
        "## Where the code is\nThe project, with the change applied, is at {}. Read anything in it. You may run its \
         build and tests there, with {} as the build folder (for example CARGO_TARGET_DIR={}), and make read-only \
         checks of live state. Change nothing: no edits, no commits, no installs into the project, no messages to \
         anyone.\n\n",
        input.work_dir.display(),
        input.build_dir.display(),
        input.build_dir.display()
    ));
    if input.delta {
        out.push_str(&format!("## The whole change\n{}\n\n", input.whole_stat));
    }
    let heading = if input.delta {
        "The fixes since the last round"
    } else {
        "The change"
    };
    out.push_str(&format!("## {heading}\n{}\n\n", input.change.stat()));
    if input.change.patch.chars().count() <= INLINE_PATCH_CHARS {
        out.push_str(&format!("```diff\n{}\n```\n\n", input.change.patch));
    } else {
        out.push_str(&format!(
            "The diff is too long to show here; read it whole at {}.\n\n",
            input.patch_file.display()
        ));
    }
    if !input.checks.is_empty() {
        out.push_str("## The project's checks this round\n");
        for c in input.checks {
            let verdict = if c.passed { "passed" } else { "FAILED" };
            out.push_str(&format!("- {} ({}): {verdict}\n", c.label, c.command));
            if !c.passed && !c.tail.is_empty() {
                out.push_str(&format!("```\n{}\n```\n", c.tail));
            }
        }
        out.push('\n');
    }
    if !input.prior_open.is_empty() {
        out.push_str("## Earlier findings still open\n");
        for f in input.prior_open {
            out.push_str(&format!(
                "- {} [{}] {} ({}). Fix asked: {}\n",
                f.id,
                serde_json::to_value(f.severity)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default(),
                f.title,
                f.evidence,
                f.fix
            ));
        }
        out.push('\n');
    }
    if !input.builder_notes.trim().is_empty() {
        out.push_str(&format!(
            "## What the builder says it did about them\n{}\n\n",
            input.builder_notes.trim()
        ));
    }
    out.push_str(input.rules);
    out.push_str(
        "\n\n## Your answer\nWhen you are done, answer with one JSON object and nothing else:\n\
         {\"findings\": [{\"severity\": \"blocker|major|minor\", \"kind\": \"defect|decision\", \"title\": \"...\", \
         \"evidence\": \"path:line\", \"repro\": \"steps or worked numbers\", \"impact\": \"...\", \"fix\": \"...\"}], \
         \"verified_ok\": [{\"checked\": \"what you checked that held\"}], \"prior\": [{\"id\": \"...\", \"state\": \
         \"fixed|open|deferred\", \"note\": \"...\"}]}\n\
         - blocker: wrong results, lost or corrupted data, a security hole, money wrong, or the build or tests fail.\n\
         - major: a real defect or regression someone will hit.\n\
         - minor: worth fixing, harmless meanwhile.\n\
         - kind \"decision\": the change needs a product or business call only the owner can make (not a defect). Give \
         the choices in `options` and the one you recommend in `recommended`.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::verify::findings::Severity;
    use crate::harness::verify::shadow::FileChange;

    fn change() -> ChangeSet {
        ChangeSet {
            files: vec![FileChange {
                status: "modified".into(),
                path: "src/total.rs".into(),
                from: None,
                added_lines: 1,
                removed_lines: 1,
            }],
            patch: "-let t = a;\n+let t = a + b;".into(),
            patch_cut: false,
        }
    }

    fn input<'a>(
        change: &'a ChangeSet,
        delta: bool,
        prior: &'a [Finding],
        checks: &'a [CheckRun],
    ) -> RoundInput<'a> {
        RoundInput {
            verification_id: "v1",
            round: if delta { 2 } else { 1 },
            delta,
            lenses: &[Lens::Correctness],
            spec: "Show the invoice total.",
            change,
            whole_stat: "M src/total.rs (+1 -1)",
            patch_file: Path::new("/data/verify/v1/r1.patch"),
            work_dir: Path::new("/data/worktrees/site/v1"),
            build_dir: Path::new("/data/verify/v1/build"),
            checks,
            prior_open: prior,
            builder_notes: "",
            rules: "## How this work is done\n- Read-only means read-only.",
        }
    }

    #[test]
    fn a_brief_carries_the_request_the_change_the_checks_and_the_rules_and_nothing_of_the_builder()
    {
        let c = change();
        let checks = vec![CheckRun {
            label: "Rust tests".into(),
            command: "cargo test".into(),
            passed: false,
            tail: "1 failed".into(),
            duration_ms: 9,
        }];
        let b = brief(Lens::Billing, &input(&c, false, &[], &checks));
        assert!(b.contains(Lens::Billing.charter()));
        assert!(b.contains("refute it"));
        assert!(b.contains("A finding you cannot reproduce is dropped"));
        assert!(b.contains("Show the invoice total."));
        assert!(b.contains("+let t = a + b;"));
        assert!(b.contains("Rust tests (cargo test): FAILED\n```\n1 failed"));
        assert!(b.contains("CARGO_TARGET_DIR=/data/verify/v1/build"));
        assert!(b.contains("Read-only means read-only."));
        assert!(
            !b.contains("round 2"),
            "a first round reviews the whole change"
        );
    }

    #[test]
    fn a_delta_brief_names_the_open_findings_and_asks_what_became_of_them() {
        let c = change();
        let prior = vec![Finding {
            id: "r1-correctness-1".into(),
            lens: "correctness".into(),
            severity: Severity::Major,
            kind: Default::default(),
            title: "b is ignored".into(),
            evidence: "src/total.rs:3".into(),
            repro: "a=1,b=2 gives 1".into(),
            impact: "wrong totals".into(),
            fix: "add b".into(),
            options: vec![],
            recommended: None,
        }];
        let mut i = input(&c, true, &prior, &[]);
        i.builder_notes = "Fixed r1-correctness-1.";
        let b = brief(Lens::Correctness, &i);
        assert!(b.contains("This is round 2"));
        assert!(b.contains("- r1-correctness-1 [major] b is ignored (src/total.rs:3)"));
        assert!(b.contains("Fixed r1-correctness-1."));
        assert!(b.contains("The fixes since the last round"));
    }

    #[test]
    fn a_long_patch_is_read_from_its_file() {
        let mut c = change();
        c.patch = "+x\n".repeat(INLINE_PATCH_CHARS);
        let b = brief(Lens::Correctness, &input(&c, false, &[], &[]));
        assert!(b.contains("read it whole at /data/verify/v1/r1.patch"));
        assert!(b.len() < INLINE_PATCH_CHARS);
    }

    #[test]
    fn findings_are_numbered_by_round_and_lens() {
        let review: Review = serde_json::from_value(serde_json::json!({
            "findings": [
                {"severity": "blocker", "title": "t", "evidence": "a:1", "repro": "r", "impact": "i", "fix": "f"},
                {"severity": "minor", "title": "u", "evidence": "a:2", "repro": "r", "impact": "i", "fix": "f"}
            ],
            "verified_ok": [{"checked": "c"}]
        }))
        .unwrap();
        let r = numbered(review, 3, Lens::Security);
        assert_eq!(
            r.findings.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            ["r3-security-1", "r3-security-2"]
        );
        assert_eq!(r.verified_ok[0].lens, "security");
    }
}
