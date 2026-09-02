//! Plan artifacts (coding harness P6.1).
//!
//! A plan is a markdown work document — the owner sees it in the Work panel
//! through the same artifact pathway as any other `.md` the employee writes —
//! with one line per step carrying a verify command. The employee can write
//! the plan, but it cannot tick a box: only `plan_check` does, and only from
//! the exit code of the step's verify command. Progress is measured, never
//! declared.
//!
//! Format (parsed and re-rendered by this module, so hand edits survive as
//! long as the step lines keep their shape):
//!
//! ```markdown
//! # Plan: Fix the login redirect
//! <!-- nebo-plan v1: steps are ticked only by plan_check, from the exit code of each verify command -->
//! - [ ] 1. Reproduce with a test (verify: `cargo test -p web redirect`)
//! - [x] 2. Fix the handler (verify: `cargo test -p web redirect`)
//!
//! Last check: 2026-09-02T14:03:11Z, 1/2 verified
//!   1. ✗ Reproduce with a test, exit 101: test redirect_keeps_query ... FAILED
//!   2. ✓ Fix the handler
//!
//! No em-dashes anywhere: the owner reads this document in the Work panel.
//! ```

pub const MARKER: &str =
    "<!-- nebo-plan v1: steps are ticked only by plan_check, from the exit code of each verify command -->";

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub n: usize,
    pub title: String,
    pub verify: String,
    pub checked: bool,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub title: String,
    pub steps: Vec<Step>,
}

/// Outcome of running one step's verify command.
#[derive(Debug, Clone)]
pub struct StepResult {
    pub n: usize,
    pub ok: bool,
    /// Exit code when known.
    pub exit: Option<i32>,
    /// First line of stderr/stdout worth showing on failure (already capped).
    pub note: String,
}

/// Build a fresh plan document.
pub fn render(title: &str, steps: &[(String, String)]) -> Result<String, String> {
    if title.trim().is_empty() {
        return Err("plan needs `title`".into());
    }
    if steps.is_empty() {
        return Err("plan needs `steps`: [{title, verify}] — every step must name a verify command".into());
    }
    let mut out = format!("# Plan: {}\n{MARKER}\n", title.trim());
    for (i, (t, v)) in steps.iter().enumerate() {
        if t.trim().is_empty() {
            return Err(format!("step {} has no title", i + 1));
        }
        if v.trim().is_empty() {
            return Err(format!(
                "step {} (\"{}\") has no verify command — a step that cannot be checked is not a step",
                i + 1,
                t.trim()
            ));
        }
        if v.contains('`') {
            return Err(format!("step {} verify command must not contain a backtick", i + 1));
        }
        if t.contains(" (verify: `") || t.contains(" — verify: `") {
            return Err(format!("step {} title must not contain the verify marker", i + 1));
        }
        out.push_str(&format!("- [ ] {}. {} (verify: `{}`)\n", i + 1, t.trim(), v.trim()));
    }
    Ok(out)
}

/// Parse the step lines out of a plan document. Anything that is not a step
/// line is left alone by `apply`.
pub fn parse(content: &str) -> Result<Plan, String> {
    if !content.contains("nebo-plan v1") {
        return Err(format!(
            "not a nebo plan (missing marker). Create one with action: \"plan\", title, steps: [{{title, verify}}]; the marker line is:\n{MARKER}"
        ));
    }
    let title = content
        .lines()
        .find_map(|l| l.strip_prefix("# Plan: "))
        .unwrap_or("")
        .trim()
        .to_string();
    let mut steps: Vec<Step> = Vec::new();
    for line in content.lines() {
        if let Some(step) = parse_step(line) {
            if steps.iter().any(|s| s.n == step.n) {
                return Err(format!(
                    "plan has two steps numbered {}: \"{}\". Renumber the steps so each N is unique.",
                    step.n,
                    line.trim()
                ));
            }
            steps.push(step);
        }
    }
    if steps.is_empty() {
        return Err("plan has no step lines of the form `- [ ] N. title (verify: `cmd`)`".into());
    }
    Ok(Plan { title, steps })
}

fn parse_step(line: &str) -> Option<Step> {
    let rest = line.strip_prefix("- [")?;
    let checked = match rest.chars().next()? {
        'x' | 'X' => true,
        ' ' => false,
        _ => return None,
    };
    let rest = rest.get(1..)?.strip_prefix("] ")?;
    let (num, rest) = rest.split_once(". ")?;
    let n: usize = num.trim().parse().ok()?;
    // Current form `title (verify: `cmd`)`; the pre-release form used an
    // em-dash separator and is still read so existing plans keep working.
    let (title, verify) = if let Some((t, v)) = rest.rsplit_once(" (verify: `") {
        (t, v.strip_suffix("`)")?)
    } else {
        let (t, v) = rest.rsplit_once(" — verify: `")?;
        (t, v.strip_suffix('`')?)
    };
    Some(Step { n, title: title.trim().to_string(), verify: verify.to_string(), checked })
}

/// Rewrite the checkboxes from `results` and replace the "Last check" block.
/// Returns the new content and how many steps became verified that were not
/// before (the measured-progress signal).
pub fn apply(content: &str, results: &[StepResult], now: &str) -> (String, usize) {
    let mut newly = 0usize;
    let mut lines: Vec<String> = Vec::new();
    for line in content.lines() {
        if line.starts_with("Last check:") {
            break; // the block is regenerated below; everything after it too
        }
        if let Some(step) = parse_step(line) {
            let r = results.iter().find(|r| r.n == step.n);
            let checked = r.map(|r| r.ok).unwrap_or(step.checked);
            if checked && !step.checked {
                newly += 1;
            }
            lines.push(format!(
                "- [{}] {}. {} (verify: `{}`)",
                if checked { 'x' } else { ' ' },
                step.n,
                step.title,
                step.verify
            ));
        } else {
            lines.push(line.to_string());
        }
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    let verified = results.iter().filter(|r| r.ok).count();
    lines.push(String::new());
    lines.push(format!("Last check: {now}, {verified}/{} verified", results.len()));
    for r in results {
        let title = parse(content)
            .ok()
            .and_then(|p| p.steps.into_iter().find(|s| s.n == r.n).map(|s| s.title))
            .unwrap_or_default();
        if r.ok {
            lines.push(format!("  {}. ✓ {}", r.n, title));
        } else {
            let exit = r.exit.map(|c| format!("exit {c}")).unwrap_or_else(|| "did not run".into());
            let note = if r.note.is_empty() { String::new() } else { format!(": {}", r.note) };
            lines.push(format!("  {}. ✗ {}, {exit}{note}", r.n, title));
        }
    }
    lines.push(String::new());
    (lines.join("\n"), newly)
}

/// Cap a stderr/stdout blob to one useful line.
pub fn first_line(s: &str, max: usize) -> String {
    let line = s
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if line.chars().count() > max {
        let cut: String = line.chars().take(max).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps() -> Vec<(String, String)> {
        vec![
            ("Reproduce with a test".into(), "cargo test -p web redirect".into()),
            ("Fix the handler".into(), "cargo test -p web".into()),
        ]
    }

    #[test]
    fn render_then_parse_round_trips() {
        let doc = render("Fix the login redirect", &steps()).unwrap();
        assert!(doc.starts_with("# Plan: Fix the login redirect\n"));
        assert!(doc.contains(MARKER));
        let plan = parse(&doc).unwrap();
        assert_eq!(plan.title, "Fix the login redirect");
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[1].verify, "cargo test -p web");
        assert!(plan.steps.iter().all(|s| !s.checked));
    }

    #[test]
    fn only_a_passing_command_ticks_a_box_and_progress_is_counted_once() {
        let doc = render("t", &steps()).unwrap();
        let results = vec![
            StepResult { n: 1, ok: false, exit: Some(101), note: "test redirect_keeps_query ... FAILED".into() },
            StepResult { n: 2, ok: true, exit: Some(0), note: String::new() },
        ];
        let (doc2, newly) = apply(&doc, &results, "2026-09-02T14:03:11Z");
        assert_eq!(newly, 1);
        let plan = parse(&doc2).unwrap();
        assert!(!plan.steps[0].checked);
        assert!(plan.steps[1].checked);
        assert!(doc2.contains("Last check: 2026-09-02T14:03:11Z, 1/2 verified"));
        assert!(doc2.contains("1. ✗ Reproduce with a test, exit 101: test redirect_keeps_query ... FAILED"));
        assert!(!doc2.contains('—'), "owner-visible document carries no em-dash:\n{doc2}");
        assert!(doc2.contains("2. ✓ Fix the handler"));

        // Re-checking with the same outcome is idempotent and counts no new progress.
        let (doc3, newly2) = apply(&doc2, &results, "2026-09-02T14:05:00Z");
        assert_eq!(newly2, 0);
        assert_eq!(doc3.matches("Last check:").count(), 1, "the block is replaced, not appended");
        // A regression unticks.
        let regress = vec![StepResult { n: 2, ok: false, exit: Some(1), note: "boom".into() }];
        let (doc4, _) = apply(&doc3, &regress, "t");
        assert!(!parse(&doc4).unwrap().steps[1].checked);
    }

    #[test]
    fn a_hand_ticked_box_does_not_survive_a_failing_check() {
        let doc = render("t", &steps()).unwrap().replace("- [ ] 1.", "- [x] 1.");
        assert!(parse(&doc).unwrap().steps[0].checked);
        let (doc2, newly) = apply(&doc, &[StepResult { n: 1, ok: false, exit: Some(2), note: String::new() }], "t");
        assert_eq!(newly, 0);
        assert!(!parse(&doc2).unwrap().steps[0].checked);
    }

    #[test]
    fn refusals_name_the_missing_piece() {
        assert!(render("", &steps()).unwrap_err().contains("title"));
        assert!(render("t", &[]).unwrap_err().contains("verify"));
        let err = render("t", &[("do it".into(), "".into())]).unwrap_err();
        assert!(err.contains("cannot be checked"), "{err}");
        assert!(parse("# just markdown\n").unwrap_err().contains("marker"));
    }

    #[test]
    fn old_em_dash_lines_still_parse_for_one_release() {
        let old = format!("# Plan: t\n{MARKER}\n- [x] 1. Old step — verify: `true`\n");
        let new = format!("# Plan: t\n{MARKER}\n- [x] 1. Old step (verify: `true`)\n");
        assert_eq!(parse(&old).unwrap().steps, parse(&new).unwrap().steps);
        // Re-rendering an old plan writes the new form.
        let (doc, _) = apply(&old, &[StepResult { n: 1, ok: true, exit: Some(0), note: String::new() }], "t");
        assert!(doc.contains("- [x] 1. Old step (verify: `true`)"), "{doc}");
    }

    #[test]
    fn duplicate_step_numbers_are_refused_with_the_line_quoted() {
        let doc = format!("# Plan: t\n{MARKER}\n- [ ] 1. a (verify: `true`)\n- [ ] 1. b (verify: `true`)\n");
        let err = parse(&doc).unwrap_err();
        assert!(err.contains("two steps numbered 1") && err.contains("1. b"), "{err}");
    }

    #[test]
    fn first_line_caps() {
        assert_eq!(first_line("\n\n  hello world  \nmore", 5), "hello…");
        assert_eq!(first_line("", 5), "");
    }
}
