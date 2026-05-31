use std::collections::HashMap;

use super::fixture::Fixture;
use super::trace::*;

pub fn print_report(fixture: &Fixture, traces: &[Trace]) {
    let n = traces.len();
    println!("\nnebo test run — {}", fixture.id);
    println!();
    println!("  Component: {}", fixture.target_component);
    println!(
        "  Model:     {}",
        traces.first().map(|t| t.model.as_str()).unwrap_or("?")
    );
    println!("  Runs:      {}", n);

    // FCSR
    let graded: Vec<_> = traces.iter().filter_map(|t| t.grade.as_ref()).collect();
    if !graded.is_empty() {
        println!();
        println!("  ═══ FIRST-CALL SUCCESS RATE ═══");
        println!();
        for (i, g) in graded.iter().enumerate() {
            print!("    Run {}: {:.0}%", i + 1, g.first_call_success_rate * 100.0);
            if i < graded.len() - 1 {
                print!("    ");
            }
        }
        println!();
        let avg_fcsr: f64 =
            graded.iter().map(|g| g.first_call_success_rate).sum::<f64>() / graded.len() as f64;
        println!("    Average: {:.0}%", avg_fcsr * 100.0);

        // Context pollution
        println!();
        println!("  ═══ CONTEXT POLLUTION ═══");
        println!();
        for (i, g) in graded.iter().enumerate() {
            print!("    Run {}: {:.2}", i + 1, g.context_pollution_score);
            if i < graded.len() - 1 {
                print!("    ");
            }
        }
        println!();
        let avg_poll: f64 =
            graded.iter().map(|g| g.context_pollution_score).sum::<f64>() / graded.len() as f64;
        let label = if avg_poll < 0.05 {
            "clean"
        } else if avg_poll < 0.3 {
            "minor"
        } else {
            "significant"
        };
        println!("    Average: {:.2} ({})", avg_poll, label);

        // Assertions table
        if let Some(first_grade) = graded.first() {
            println!();
            println!("  ═══ ASSERTIONS ═══");
            println!(
                "  {:<24} {}  Rate",
                "Assertion",
                (1..=n)
                    .map(|i| format!("Run {:<2}", i))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            println!("  {}", "─".repeat(24 + n * 6 + 8));

            for assertion in &first_grade.assertions {
                let results: Vec<&str> = graded
                    .iter()
                    .map(|g| {
                        g.assertions
                            .iter()
                            .find(|a| a.id == assertion.id)
                            .map(|a| if a.passed { " ✓  " } else { " ✗  " })
                            .unwrap_or(" ?  ")
                    })
                    .collect();
                let pass_count = graded
                    .iter()
                    .filter(|g| {
                        g.assertions
                            .iter()
                            .any(|a| a.id == assertion.id && a.passed)
                    })
                    .count();
                let rate = if n > 0 {
                    pass_count * 100 / n
                } else {
                    0
                };
                println!(
                    "  {:<24} {} {:>3}%",
                    truncate(&assertion.id, 24),
                    results.join(" "),
                    rate
                );
            }
        }
    }

    // Cost metrics (always shown)
    println!();
    println!("  ═══ COST ═══");
    println!();
    let avg_tokens: usize = traces.iter().map(|t| t.metrics.total_tokens).sum::<usize>() / n.max(1);
    let avg_tools: f64 = traces.iter().map(|t| t.metrics.total_tool_calls).sum::<usize>() as f64
        / n.max(1) as f64;
    let avg_latency: u64 =
        traces.iter().map(|t| t.metrics.total_latency_ms).sum::<u64>() / n.max(1) as u64;
    println!(
        "    Avg tokens: {}  |  Avg tool calls: {:.1}  |  Avg latency: {}ms",
        format_number(avg_tokens),
        avg_tools,
        format_number(avg_latency as usize)
    );

    // Ideal comparison
    if let Some(ideal_calls) = fixture.ideal_behavior.tool_calls {
        let delta = avg_tools as isize - ideal_calls as isize;
        let sign = if delta > 0 { "+" } else { "" };
        println!(
            "    vs ideal: {} tool calls ({}{})",
            ideal_calls, sign, delta
        );
    }
    println!();
}

pub fn print_comparison(baseline: &[Trace], candidate: &[Trace]) {
    println!();
    println!("  ═══ COMPARISON ═══");
    println!();
    println!(
        "    {:<22} {:<18} {:<18} {}",
        "Metric", "Baseline", "Candidate", "Delta"
    );
    println!("    {}", "─".repeat(70));

    let b_graded: Vec<_> = baseline.iter().filter_map(|t| t.grade.as_ref()).collect();
    let c_graded: Vec<_> = candidate.iter().filter_map(|t| t.grade.as_ref()).collect();

    if !b_graded.is_empty() && !c_graded.is_empty() {
        let b_fcsr =
            b_graded.iter().map(|g| g.first_call_success_rate).sum::<f64>() / b_graded.len() as f64;
        let c_fcsr =
            c_graded.iter().map(|g| g.first_call_success_rate).sum::<f64>() / c_graded.len() as f64;
        print_delta_row("First-call success", &format!("{:.0}%", b_fcsr * 100.0), &format!("{:.0}%", c_fcsr * 100.0), (c_fcsr - b_fcsr) * 100.0, "%");

        let b_poll = b_graded.iter().map(|g| g.context_pollution_score).sum::<f64>()
            / b_graded.len() as f64;
        let c_poll = c_graded.iter().map(|g| g.context_pollution_score).sum::<f64>()
            / c_graded.len() as f64;
        print_delta_row("Context pollution", &format!("{:.2}", b_poll), &format!("{:.2}", c_poll), c_poll - b_poll, "");
    }

    let b_tokens =
        baseline.iter().map(|t| t.metrics.total_tokens).sum::<usize>() / baseline.len().max(1);
    let c_tokens =
        candidate.iter().map(|t| t.metrics.total_tokens).sum::<usize>() / candidate.len().max(1);
    let token_pct = if b_tokens > 0 {
        ((c_tokens as f64 - b_tokens as f64) / b_tokens as f64) * 100.0
    } else {
        0.0
    };
    print_delta_row(
        "Avg tokens",
        &format_number(b_tokens),
        &format_number(c_tokens),
        token_pct,
        "%",
    );

    let b_calls =
        baseline.iter().map(|t| t.metrics.total_tool_calls).sum::<usize>() as f64 / baseline.len().max(1) as f64;
    let c_calls =
        candidate.iter().map(|t| t.metrics.total_tool_calls).sum::<usize>() as f64 / candidate.len().max(1) as f64;
    let call_pct = if b_calls > 0.0 {
        ((c_calls - b_calls) / b_calls) * 100.0
    } else {
        0.0
    };
    print_delta_row("Avg tool calls", &format!("{:.1}", b_calls), &format!("{:.1}", c_calls), call_pct, "%");

    println!();
}

pub fn print_json_report(traces: &[Trace]) {
    let json = serde_json::to_string_pretty(traces).unwrap_or_else(|e| format!("{{\"error\": \"{}\"}}", e));
    println!("{}", json);
}

fn print_delta_row(label: &str, baseline: &str, candidate: &str, delta: f64, unit: &str) {
    let sign = if delta > 0.0 { "+" } else { "" };
    println!(
        "    {:<22} {:<18} {:<18} {}{:.0}{}",
        label, baseline, candidate, sign, delta, unit
    );
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max - 1])
    }
}

fn format_number(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{},{:03}", n / 1_000, n % 1_000)
    } else {
        n.to_string()
    }
}

/// Welch's t-test for unequal variances. Returns the two-tailed p-value approximation.
pub fn welch_t_test(a: &StatSummary, b: &StatSummary) -> f64 {
    if a.n < 2 || b.n < 2 {
        return 1.0;
    }
    let sa2 = a.std_dev.powi(2);
    let sb2 = b.std_dev.powi(2);
    let na = a.n as f64;
    let nb = b.n as f64;

    let se = (sa2 / na + sb2 / nb).sqrt();
    if se < 1e-12 {
        return if (a.mean - b.mean).abs() < 1e-12 { 1.0 } else { 0.0 };
    }

    let t = (a.mean - b.mean) / se;

    // Welch-Satterthwaite degrees of freedom
    let num = (sa2 / na + sb2 / nb).powi(2);
    let denom = (sa2 / na).powi(2) / (na - 1.0) + (sb2 / nb).powi(2) / (nb - 1.0);
    let df = num / denom;

    // Approximate two-tailed p-value using the normal distribution for df >= 30,
    // otherwise use a conservative t-distribution approximation
    approx_two_tailed_p(t.abs(), df)
}

/// Conservative p-value approximation without external stats crate.
/// Uses the relationship between t and normal distributions.
fn approx_two_tailed_p(t: f64, df: f64) -> f64 {
    // For large df, t approaches normal. For smaller df, we use a correction.
    // Hill's approximation: convert t to a z-score adjusted for df
    let a = df / (df + t * t);
    // Regularized incomplete beta function approximation for I_x(a, b)
    // where x = df/(df+t^2), a = df/2, b = 1/2
    // For our purposes, a simple series expansion works
    regularized_beta(a, df / 2.0, 0.5)
}

/// Regularized incomplete beta function I_x(a, b) via continued fraction.
/// This gives the two-tailed p-value for the t-distribution.
fn regularized_beta(x: f64, a: f64, b: f64) -> f64 {
    if x <= 0.0 { return 0.0; }
    if x >= 1.0 { return 1.0; }

    // Use the continued fraction representation (Lentz's method)
    let ln_beta = ln_gamma(a) + ln_gamma(b) - ln_gamma(a + b);
    let front = (a * x.ln() + b * (1.0 - x).ln() - ln_beta).exp() / a;

    // Modified Lentz's continued fraction
    let mut f = 1.0;
    let mut c = 1.0;
    let mut d;
    let max_iter = 200;
    let eps = 1e-14;

    for m in 0..max_iter {
        let m_f64 = m as f64;
        let numerator = if m == 0 {
            1.0
        } else if m % 2 == 0 {
            let k = m_f64 / 2.0;
            k * (b - k) * x / ((a + 2.0 * k - 1.0) * (a + 2.0 * k))
        } else {
            let k = (m_f64 - 1.0) / 2.0;
            -(a + k) * (a + b + k) * x / ((a + 2.0 * k) * (a + 2.0 * k + 1.0))
        };

        d = 1.0 + numerator;
        if d.abs() < 1e-30 { d = 1e-30; }
        d = 1.0 / d;

        c = 1.0 + numerator / c;
        if c.abs() < 1e-30 { c = 1e-30; }

        let delta = c * d;
        f *= delta;

        if (delta - 1.0).abs() < eps {
            break;
        }
    }

    front * (f - 1.0)
}

/// Lanczos approximation for ln(Gamma(x))
fn ln_gamma(x: f64) -> f64 {
    let coeffs = [
        76.18009172947146,
        -86.50532032941677,
        24.01409824083091,
        -1.231739572450155,
        0.1208650973866179e-2,
        -0.5395239384953e-5,
    ];
    let y = x;
    let mut tmp = x + 5.5;
    tmp -= (x + 0.5) * tmp.ln();
    let mut ser = 1.000000000190015;
    for (j, c) in coeffs.iter().enumerate() {
        ser += c / (y + 1.0 + j as f64);
    }
    -tmp + (2.5066282746310005 * ser / x).ln()
}

const NOISE_THRESHOLD: f64 = 0.05;
const SIGNIFICANCE_LEVEL: f64 = 0.05;

/// Compare baseline and candidate fixture scores, produce comparisons and a verdict.
pub fn compute_experiment_result(
    metadata: ExperimentMetadata,
    baseline_scores: &[FixtureScores],
    candidate_scores: &[FixtureScores],
) -> ExperimentResult {
    let mut comparisons = Vec::new();
    let mut any_regression = false;
    let mut any_improvement = false;
    let mut all_significant = true;

    // Build lookup for baseline by fixture_id
    let baseline_map: HashMap<&str, &FixtureScores> = baseline_scores
        .iter()
        .map(|s| (s.fixture_id.as_str(), s))
        .collect();

    for candidate in candidate_scores {
        let baseline = match baseline_map.get(candidate.fixture_id.as_str()) {
            Some(b) => b,
            None => continue,
        };

        let b_stat = StatSummary::from_values(&baseline.fcsr_values);
        let c_stat = StatSummary::from_values(&candidate.fcsr_values);
        let delta = c_stat.mean - b_stat.mean;
        let p_value = welch_t_test(&b_stat, &c_stat);
        let significant = p_value < SIGNIFICANCE_LEVEL;
        let regressed = delta < -NOISE_THRESHOLD && significant;

        if regressed {
            any_regression = true;
        }
        if delta > NOISE_THRESHOLD && significant {
            any_improvement = true;
        }
        if !significant {
            all_significant = false;
        }

        comparisons.push(ExperimentComparison {
            fixture_id: candidate.fixture_id.clone(),
            baseline_fcsr: b_stat,
            candidate_fcsr: c_stat,
            fcsr_delta: delta,
            fcsr_p_value: p_value,
            significant,
            regressed,
        });
    }

    let (verdict, reason) = if any_regression {
        (Verdict::Revert, format!(
            "Regression detected in {} fixture(s)",
            comparisons.iter().filter(|c| c.regressed).count()
        ))
    } else if any_improvement && all_significant {
        (Verdict::Ship, "Target improved, no regressions, all results significant".to_string())
    } else if any_improvement {
        (Verdict::Inconclusive, "Improvement detected but not all results statistically significant (p > 0.05). Run more trials.".to_string())
    } else {
        (Verdict::Inconclusive, "No significant change detected. Run more trials or check overrides.".to_string())
    };

    ExperimentResult {
        metadata,
        fixture_scores: candidate_scores.to_vec(),
        comparisons,
        verdict,
        verdict_reason: reason,
    }
}

/// Print the experiment result with statistical analysis.
pub fn print_experiment_result(result: &ExperimentResult) {
    println!();
    println!("  ═══ EXPERIMENT: {} ═══", result.metadata.name);
    println!("  Git: {} ({})", &result.metadata.git_commit[..result.metadata.git_commit.len().min(8)], result.metadata.git_branch);
    println!("  Runs/fixture: {}", result.metadata.runs_per_fixture);
    if !result.metadata.overrides.is_empty() {
        println!("  Overrides: {}", result.metadata.overrides.join(", "));
    }
    println!();

    if !result.comparisons.is_empty() {
        println!("  {:<24} {:<10} {:<10} {:<10} {:<10} {}", "Fixture", "Baseline", "Candidate", "Delta", "p-value", "Status");
        println!("  {}", "─".repeat(80));

        for cmp in &result.comparisons {
            let status = if cmp.regressed {
                "REGRESSED"
            } else if cmp.significant && cmp.fcsr_delta > NOISE_THRESHOLD {
                "IMPROVED"
            } else if cmp.significant {
                "CHANGED"
            } else {
                "~"
            };

            println!(
                "  {:<24} {:<10.0}% {:<10.0}% {:<+10.0}% {:<10.4} {}",
                truncate(&cmp.fixture_id, 24),
                cmp.baseline_fcsr.mean * 100.0,
                cmp.candidate_fcsr.mean * 100.0,
                cmp.fcsr_delta * 100.0,
                cmp.fcsr_p_value,
                status,
            );
        }
    }

    println!();
    println!("  VERDICT: {}", result.verdict);
    println!("  Reason:  {}", result.verdict_reason);
    println!();
}

/// Append an experiment result as a single JSON line to history.jsonl
pub fn append_history(dir: &std::path::Path, result: &ExperimentResult) -> Result<(), String> {
    use std::io::Write;
    std::fs::create_dir_all(dir).map_err(|e| format!("create dir: {}", e))?;
    let path = dir.join("history.jsonl");
    let line = serde_json::to_string(result).map_err(|e| format!("serialize: {}", e))?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("open {}: {}", path.display(), e))?;
    writeln!(file, "{}", line).map_err(|e| format!("write: {}", e))?;
    Ok(())
}
