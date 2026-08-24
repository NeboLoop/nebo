//! Program-evaluable assertion checks (WS1).
//!
//! A `check`-bearing assertion is decided here, deterministically, from the
//! recorded trace — before and independent of the LLM judge. Two invariants:
//!
//! - **Never fail open.** A malformed matcher is a fixture-authoring error:
//!   the run fails with a diagnostic. The historical failure mode this kills
//!   is a mechanically-checkable claim ("tool_calls: 1") silently graded by a
//!   model that agrees with itself.
//! - **Evidence over verdicts.** Every failure names what the trace actually
//!   contained, so a red row is directly actionable.

use super::fixture::{Assertion, Check, Fixture};
use super::trace::{AssertionResult, Trace, TracedToolCall};

pub const MODE_VERIFIED: &str = "verified";
pub const MODE_JUDGED: &str = "judged";

/// Evaluate every `check`-bearing assertion of the fixture against a trace.
/// `Err` = malformed matcher (diagnostic; the caller must fail the run).
pub fn evaluate_fixture_checks(
    fixture: &Fixture,
    trace: &Trace,
) -> Result<Vec<AssertionResult>, String> {
    let mut out = Vec::new();
    for assertion in fixture
        .prompt_assertions
        .all()
        .into_iter()
        .chain(fixture.integrated_assertions.iter())
    {
        if let Some(check) = &assertion.check {
            let (passed, evidence) = evaluate(check, trace)
                .map_err(|e| format!("assertion '{}': malformed check: {}", assertion.id, e))?;
            out.push(AssertionResult {
                id: assertion.id.clone(),
                passed,
                evidence,
                mode: MODE_VERIFIED.to_string(),
            });
        }
    }
    Ok(out)
}

/// The assertions the LLM judge should still see: prose-only ones.
pub fn judged_assertions(fixture: &Fixture) -> Vec<Assertion> {
    fixture
        .prompt_assertions
        .all()
        .into_iter()
        .chain(fixture.integrated_assertions.iter())
        .filter(|a| a.check.is_none())
        .cloned()
        .collect()
}

/// Evaluate one check. `Ok((passed, evidence))`; `Err(diagnostic)` when the
/// matcher itself is invalid.
fn evaluate(check: &Check, trace: &Trace) -> Result<(bool, String), String> {
    validate(check)?;

    let mut evidence: Vec<String> = Vec::new();

    // Trace-level criteria.
    if let Some(want) = check.tool_calls {
        let got = trace.metrics.total_tool_calls;
        if got != want {
            return Ok((false, format!("expected {} tool call(s), trace has {}", want, got)));
        }
        evidence.push(format!("tool_calls == {}", want));
    }
    if let Some(max) = check.max_tool_calls {
        let got = trace.metrics.total_tool_calls;
        if got > max {
            return Ok((false, format!("expected ≤{} tool call(s), trace has {}", max, got)));
        }
        evidence.push(format!("tool_calls {} ≤ {}", got, max));
    }
    if let Some(max) = check.max_total_tokens {
        let got = trace.metrics.total_tokens;
        if got > max {
            return Ok((false, format!("expected ≤{} total tokens, trace used {}", max, got)));
        }
        evidence.push(format!("total_tokens {} ≤ {}", got, max));
    }

    // Call-level criteria.
    let ordinal = if check.first_call { Some(1) } else { check.call };
    if let Some(n) = ordinal {
        let Some(call) = trace.tool_calls.get(n - 1) else {
            return Ok((false, format!("no call #{} — trace has {} call(s)", n, trace.tool_calls.len())));
        };
        match check_one_call(check, call)? {
            Ok(ev) => evidence.push(ev),
            Err(why) => return Ok((false, format!("call #{}: {}", n, why))),
        }
    } else if !check.tool.is_empty() {
        // No ordinal: SOME call by one of the named tools must satisfy the
        // arg predicates.
        let candidates: Vec<&TracedToolCall> = trace
            .tool_calls
            .iter()
            .filter(|c| check.tool.iter().any(|t| t == &c.tool))
            .collect();
        if candidates.is_empty() {
            let seen: Vec<&str> = trace.tool_calls.iter().map(|c| c.tool.as_str()).collect();
            return Ok((false, format!(
                "no call by {:?} — tools called: {:?}",
                check.tool, seen
            )));
        }
        let mut last_why = String::new();
        let mut hit = None;
        for c in &candidates {
            match check_one_call(check, c)? {
                Ok(ev) => {
                    hit = Some(format!("call #{}: {}", c.sequence, ev));
                    break;
                }
                Err(why) => last_why = format!("call #{}: {}", c.sequence, why),
            }
        }
        match hit {
            Some(ev) => evidence.push(ev),
            None => return Ok((false, last_why)),
        }
    }

    Ok((true, evidence.join("; ")))
}

/// Check tool-name and arg predicates against one call.
/// Outer `Err` = malformed matcher; inner `Err` = predicate failed (why).
fn check_one_call(
    check: &Check,
    call: &TracedToolCall,
) -> Result<Result<String, String>, String> {
    if !check.tool.is_empty() && !check.tool.iter().any(|t| t == &call.tool) {
        return Ok(Err(format!(
            "tool is '{}', expected one of {:?}",
            call.tool, check.tool
        )));
    }
    let mut ev = if check.tool.is_empty() {
        format!("tool '{}'", call.tool)
    } else {
        format!("tool '{}' ∈ {:?}", call.tool, check.tool)
    };

    if let Some(arg_path) = &check.arg {
        let value = lookup(&call.arguments, arg_path);
        let Some(value) = value else {
            let keys = call
                .arguments
                .as_object()
                .map(|o| o.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            return Ok(Err(format!("arg '{}' absent — args present: {:?}", arg_path, keys)));
        };

        if let Some(want) = &check.equals {
            let want_json = yaml_to_json(want);
            if !json_eq(&want_json, value) {
                return Ok(Err(format!(
                    "arg '{}' is {}, expected {}",
                    arg_path, value, want_json
                )));
            }
            ev.push_str(&format!(", {} == {}", arg_path, want_json));
        }
        if let Some(needle) = &check.contains {
            let hay = stringify(value);
            if !hay.contains(needle.as_str()) {
                return Ok(Err(format!(
                    "arg '{}' ({}) does not contain '{}'",
                    arg_path, hay, needle
                )));
            }
            ev.push_str(&format!(", {} contains '{}'", arg_path, needle));
        }
        if let Some(pattern) = &check.matches {
            let re = regex::Regex::new(pattern)
                .map_err(|e| format!("invalid regex '{}': {}", pattern, e))?;
            let hay = stringify(value);
            if !re.is_match(&hay) {
                return Ok(Err(format!(
                    "arg '{}' ({}) does not match /{}/",
                    arg_path, hay, pattern
                )));
            }
            ev.push_str(&format!(", {} matches /{}/", arg_path, pattern));
        }
        if check.equals.is_none() && check.contains.is_none() && check.matches.is_none() {
            // exists (explicit or the only remaining reading of `arg:`)
            ev.push_str(&format!(", {} present", arg_path));
        }
    }
    Ok(Ok(ev))
}

fn validate(check: &Check) -> Result<(), String> {
    let has_call_selector =
        check.first_call || check.call.is_some() || !check.tool.is_empty();
    let has_arg_predicate = check.arg.is_some();
    let has_trace_predicate = check.tool_calls.is_some()
        || check.max_tool_calls.is_some()
        || check.max_total_tokens.is_some();

    if !has_call_selector && !has_trace_predicate {
        return Err("check has no criteria (need call/first_call/tool, or a trace-level predicate)".into());
    }
    if has_arg_predicate && !has_call_selector {
        return Err("arg predicates need a call selector (call, first_call, or tool)".into());
    }
    if let Some(0) = check.call {
        return Err("call is 1-based; 0 is invalid".into());
    }
    if (check.equals.is_some() || check.contains.is_some() || check.matches.is_some() || check.exists)
        && check.arg.is_none()
    {
        return Err("equals/contains/matches/exists require an `arg`".into());
    }
    if let Some(p) = &check.matches {
        regex::Regex::new(p).map_err(|e| format!("invalid regex '{}': {}", p, e))?;
    }
    Ok(())
}

/// Dot-path lookup into a JSON value.
fn lookup<'a>(v: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut cur = v;
    for seg in path.split('.') {
        cur = cur.get(seg)?;
    }
    if cur.is_null() { None } else { Some(cur) }
}

fn stringify(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn yaml_to_json(y: &serde_yaml::Value) -> serde_json::Value {
    serde_json::to_value(y).unwrap_or(serde_json::Value::Null)
}

/// Equality with numeric coercion (YAML `1` vs traced `1.0`).
fn json_eq(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    if let (Some(x), Some(y)) = (a.as_f64(), b.as_f64()) {
        return (x - y).abs() < f64::EPSILON;
    }
    a == b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::trace::*;

    fn trace_with(calls: Vec<(&str, serde_json::Value)>, tokens: usize) -> Trace {
        let tool_calls: Vec<TracedToolCall> = calls
            .into_iter()
            .enumerate()
            .map(|(i, (tool, args))| TracedToolCall {
                sequence: i + 1,
                tool: tool.to_string(),
                arguments: args,
                response: TracedToolResponse {
                    content: String::new(),
                    is_error: false,
                    char_count: 0,
                },
                latency_ms: 0,
            })
            .collect();
        let n = tool_calls.len();
        Trace {
            fixture_id: "t".into(),
            run_id: "run-1".into(),
            model: "m".into(),
            timestamp: String::new(),
            overrides: vec![],
            tool_calls,
            final_response: TracedResponse::default(),
            metrics: TraceMetrics {
                total_tool_calls: n,
                total_tokens: tokens,
                ..Default::default()
            },
            grade: None,
        }
    }

    fn check(yaml: &str) -> Check {
        serde_yaml::from_str(yaml).expect("check yaml")
    }

    #[test]
    fn the_prd_example_verifies_and_fails_on_corruption() {
        // check: { call: 1, tool: [os, file_edit], arg: old_string, equals: "Hello" }
        let c = check(r#"{ call: 1, tool: [os, file_edit], arg: old_string, equals: "Hello" }"#);
        let good = trace_with(
            vec![("os", serde_json::json!({"old_string": "Hello", "new_string": "Goodbye"}))],
            900,
        );
        assert!(evaluate(&c, &good).unwrap().0);

        // Deliberately corrupted trace (wrong arg): fails with evidence, no judge involved.
        let bad = trace_with(vec![("os", serde_json::json!({"old_string": "Hi"}))], 900);
        let (passed, why) = evaluate(&c, &bad).unwrap();
        assert!(!passed);
        assert!(why.contains("old_string"), "evidence names the arg: {}", why);
    }

    #[test]
    fn tool_count_and_token_ceiling() {
        let t = trace_with(vec![("os", serde_json::json!({}))], 1400);
        assert!(evaluate(&check("{ tool_calls: 1 }"), &t).unwrap().0);
        let (p, why) = evaluate(&check("{ tool_calls: 2 }"), &t).unwrap();
        assert!(!p && why.contains("trace has 1"));
        assert!(evaluate(&check("{ max_total_tokens: 1500 }"), &t).unwrap().0);
        assert!(!evaluate(&check("{ max_total_tokens: 1000 }"), &t).unwrap().0);
    }

    #[test]
    fn some_call_by_tool_semantics() {
        let t = trace_with(
            vec![
                ("web", serde_json::json!({"url": "https://x.test"})),
                ("os", serde_json::json!({"path": "/tmp/a.txt"})),
            ],
            0,
        );
        // any os call with path containing /tmp
        let c = check(r#"{ tool: os, arg: path, contains: "/tmp" }"#);
        assert!(evaluate(&c, &t).unwrap().0);
        // no such tool at all → failure names what WAS called
        let (p, why) = evaluate(&check("{ tool: organizer }"), &t).unwrap();
        assert!(!p && why.contains("web"));
    }

    #[test]
    fn first_call_ordinal_and_regex() {
        let t = trace_with(vec![("os", serde_json::json!({"path": "/tmp/nebo-greeting.txt"}))], 0);
        let c = check(r#"{ first_call: true, tool: os, arg: path, matches: "^/tmp/.*\\.txt$" }"#);
        assert!(evaluate(&c, &t).unwrap().0);
        let (p, _) = evaluate(&check("{ call: 2, tool: os }"), &t).unwrap();
        assert!(!p, "missing ordinal is a failure, not an error");
    }

    #[test]
    fn malformed_matchers_never_fail_open() {
        // no criteria at all
        assert!(evaluate(&check("{}"), &trace_with(vec![], 0)).is_err());
        // arg predicate with no call selector
        assert!(evaluate(&check(r#"{ arg: path, equals: "x" }"#), &trace_with(vec![], 0)).is_err());
        // equals without arg
        assert!(evaluate(&check(r#"{ first_call: true, equals: "x" }"#), &trace_with(vec![], 0)).is_err());
        // bad regex
        assert!(evaluate(&check(r#"{ first_call: true, arg: p, matches: "(" }"#), &trace_with(vec![], 0)).is_err());
        // call: 0
        assert!(evaluate(&check("{ call: 0 }"), &trace_with(vec![], 0)).is_err());
    }

    #[test]
    fn numeric_coercion_in_equals() {
        let t = trace_with(vec![("os", serde_json::json!({"count": 1.0}))], 0);
        assert!(evaluate(&check("{ first_call: true, arg: count, equals: 1 }"), &t).unwrap().0);
    }
}
