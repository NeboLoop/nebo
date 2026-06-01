use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::engine;
use super::fixture::{self, Fixture};
use super::reporter;
use super::trace::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizeConfig {
    pub alpha: f64,
    pub beta: f64,
    pub mutations_per_round: usize,
    pub runs_per_mutation: usize,
    pub max_rounds: usize,
    pub convergence_threshold: f64,
    pub convergence_patience: usize,
    pub noise_threshold: f64,
    pub significance_level: f64,
    pub auto_apply: bool,
    pub budget_ceiling_tokens: usize,
}

impl Default for OptimizeConfig {
    fn default() -> Self {
        Self {
            alpha: 0.1,
            beta: 0.001,
            mutations_per_round: 5,
            runs_per_mutation: 10,
            max_rounds: 50,
            convergence_threshold: 0.01,
            convergence_patience: 5,
            noise_threshold: 0.05,
            significance_level: 0.05,
            auto_apply: false,
            budget_ceiling_tokens: 500_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mutation {
    pub id: String,
    pub hypothesis: String,
    pub strategy: String,
    pub target_component: String,
    pub diff: String,
    pub full_text: String,
    pub target_fixture: String,
    pub expected_impact: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizeRoundResult {
    pub round: usize,
    pub target_component: String,
    pub mutations: Vec<MutationResult>,
    pub best_mutation: Option<String>,
    pub verdict: Verdict,
    pub verdict_reason: String,
    pub baseline_suite_loss: f64,
    pub best_candidate_loss: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationResult {
    pub mutation: Mutation,
    pub experiment: ExperimentResult,
    pub suite_loss: f64,
}

pub fn compute_loss(
    fcsr: f64,
    pollution: f64,
    token_cost_normalized: f64,
    config: &OptimizeConfig,
) -> f64 {
    (1.0 - fcsr) + config.alpha * pollution + config.beta * token_cost_normalized
}

pub fn compute_suite_loss(scores: &[FixtureScores], config: &OptimizeConfig) -> f64 {
    if scores.is_empty() {
        return 1.0;
    }
    let losses: Vec<f64> = scores
        .iter()
        .map(|s| {
            let fcsr = mean(&s.fcsr_values);
            let pollution = mean(&s.pollution_values);
            let token_norm =
                mean_usize(&s.token_counts) as f64 / config.budget_ceiling_tokens as f64;
            compute_loss(fcsr, pollution, token_norm, config)
        })
        .collect();
    mean(&losses)
}

fn mean(vals: &[f64]) -> f64 {
    if vals.is_empty() {
        0.0
    } else {
        vals.iter().sum::<f64>() / vals.len() as f64
    }
}

fn mean_usize(vals: &[usize]) -> f64 {
    if vals.is_empty() {
        0.0
    } else {
        vals.iter().sum::<usize>() as f64 / vals.len() as f64
    }
}

pub fn identify_worst_component(scores: &[FixtureScores], fixtures: &[Fixture]) -> Option<String> {
    let fixture_map: HashMap<&str, &Fixture> =
        fixtures.iter().map(|f| (f.id.as_str(), f)).collect();

    let mut component_losses: HashMap<String, Vec<f64>> = HashMap::new();

    for score in scores {
        if let Some(fix) = fixture_map.get(score.fixture_id.as_str()) {
            let fcsr = mean(&score.fcsr_values);
            let loss = 1.0 - fcsr;
            component_losses
                .entry(fix.target_component.clone())
                .or_default()
                .push(loss);
        }
    }

    component_losses
        .into_iter()
        .max_by(|a, b| mean(&a.1).partial_cmp(&mean(&b.1)).unwrap())
        .map(|(component, _)| component)
}

pub fn build_mutator_prompt(
    section_text: &str,
    component: &str,
    failures: &[(String, String)],
    history: &[OptimizeRoundResult],
    n: usize,
) -> String {
    let mut prompt = format!(
        r#"You are optimizing a prompt component for an AI agent platform called Nebo.

Nebo uses a tool system where the model calls tools via a STRAP pattern:
tool(resource: "x", action: "y", param: "value")

The model reads the prompt section below and uses it to decide how to call tools.
Your job: modify the section so the model calls tools correctly on the FIRST attempt.

CURRENT PROMPT SECTION (component: {component}):
```
{section_text}
```

"#
    );

    if !failures.is_empty() {
        prompt.push_str("FIXTURE FAILURES THIS SECTION IS INVOLVED IN:\n");
        for (fixture_id, description) in failures {
            prompt.push_str(&format!("- {}: {}\n", fixture_id, description));
        }
        prompt.push('\n');
    }

    if !history.is_empty() {
        prompt.push_str("LOSS HISTORY (recent experiments on this section):\n");
        for round in history.iter().rev().take(10) {
            let best = round
                .best_candidate_loss
                .map(|l| format!("{:.3}", l))
                .unwrap_or_else(|| "n/a".to_string());
            prompt.push_str(&format!(
                "- Round {}: baseline={:.3}, best_candidate={}, verdict={}\n",
                round.round, round.baseline_suite_loss, best, round.verdict
            ));
        }
        prompt.push('\n');
    }

    prompt.push_str(&format!(
        r#"Generate {n} variations of this section. Each variation should:
1. Address the specific failure mode described above
2. Change as little as possible (minimize blast radius)
3. Be testable — the change should produce a measurable FCSR delta

MUTATION STRATEGIES (cycle through these):
1. Directive insertion — add one imperative sentence addressing the failure
2. Reordering — move a behavioral rule higher/lower in the doc
3. Simplification — remove words/sentences to reduce cognitive load
4. Example addition — add a concrete example of correct behavior
5. Negative example — add "do NOT do X" for the observed failure pattern
6. Consolidation — merge two related rules into one clearer rule
7. Vocabulary alignment — change terms to match common LLM training data (file_read, bash, grep style names)

Return ONLY valid JSON (no markdown fences, no commentary):
[
  {{
    "id": "v1",
    "hypothesis": "Why this change should improve FCSR",
    "strategy": "directive_insertion|reordering|simplification|example_addition|negative_example|consolidation|vocabulary_alignment",
    "diff": "Brief description of what changed",
    "full_text": "The complete replacement section text",
    "target_fixture": "fixture-id this targets (or 'all')",
    "expected_impact": "Expected FCSR change"
  }}
]
"#
    ));

    prompt
}

pub fn parse_mutations(
    json_str: &str,
    component: &str,
) -> Result<Vec<Mutation>, String> {
    let trimmed = json_str.trim();
    let json_body = if let Some(start) = trimmed.find('[') {
        if let Some(end) = trimmed.rfind(']') {
            &trimmed[start..=end]
        } else {
            trimmed
        }
    } else {
        trimmed
    };

    let raw: Vec<serde_json::Value> =
        serde_json::from_str(json_body).map_err(|e| format!("parse mutations JSON: {}", e))?;

    let mut mutations = Vec::new();
    for item in raw {
        mutations.push(Mutation {
            id: item["id"].as_str().unwrap_or("v?").to_string(),
            hypothesis: item["hypothesis"].as_str().unwrap_or("").to_string(),
            strategy: item["strategy"].as_str().unwrap_or("unknown").to_string(),
            target_component: component.to_string(),
            diff: item["diff"].as_str().unwrap_or("").to_string(),
            full_text: item["full_text"].as_str().unwrap_or("").to_string(),
            target_fixture: item["target_fixture"].as_str().unwrap_or("all").to_string(),
            expected_impact: item["expected_impact"].as_str().unwrap_or("").to_string(),
        });
    }

    Ok(mutations)
}

pub async fn generate_mutations(
    component: &str,
    section_text: &str,
    failures: &[(String, String)],
    history: &[OptimizeRoundResult],
    n: usize,
    server: &str,
    grader_model: &str,
) -> Result<Vec<Mutation>, String> {
    let prompt = build_mutator_prompt(section_text, component, failures, history, n);

    let response = super::grader::call_grader_raw(server, grader_model, &prompt).await?;

    parse_mutations(&response, component)
}

pub async fn run_optimization_round(
    round: usize,
    target: &str,
    fixtures: &[Fixture],
    baseline_scores: &[FixtureScores],
    config: &OptimizeConfig,
    server: &str,
    model: Option<&str>,
    grader_model: &str,
    output_dir: &Path,
    history: &[OptimizeRoundResult],
) -> Result<OptimizeRoundResult, String> {
    let baseline_loss = compute_suite_loss(baseline_scores, config);
    info!(round, target, baseline_loss, "starting optimization round");

    let section_text = crate::prompt::strap_tool_doc(target)
        .ok_or_else(|| format!("no STRAP doc for component '{}'", target))?;

    let failures: Vec<(String, String)> = baseline_scores
        .iter()
        .filter(|s| mean(&s.fcsr_values) < 0.9)
        .filter_map(|s| {
            fixtures
                .iter()
                .find(|f| f.id == s.fixture_id && f.target_component == target)
                .map(|f| {
                    let fcsr = mean(&s.fcsr_values);
                    (
                        f.id.clone(),
                        format!("FCSR={:.0}%, {} avg calls", fcsr * 100.0, mean_usize(&s.tool_call_counts)),
                    )
                })
        })
        .collect();

    if failures.is_empty() {
        return Ok(OptimizeRoundResult {
            round,
            target_component: target.to_string(),
            mutations: Vec::new(),
            best_mutation: None,
            verdict: Verdict::Inconclusive,
            verdict_reason: format!("All fixtures for component '{}' already at 90%+ FCSR", target),
            baseline_suite_loss: baseline_loss,
            best_candidate_loss: None,
        });
    }

    println!("  Generating {} mutations for '{}'...", config.mutations_per_round, target);
    let mutations = generate_mutations(
        target,
        section_text,
        &failures,
        history,
        config.mutations_per_round,
        server,
        grader_model,
    )
    .await?;

    if mutations.is_empty() {
        return Ok(OptimizeRoundResult {
            round,
            target_component: target.to_string(),
            mutations: Vec::new(),
            best_mutation: None,
            verdict: Verdict::Inconclusive,
            verdict_reason: "Mutator produced no valid mutations".to_string(),
            baseline_suite_loss: baseline_loss,
            best_candidate_loss: None,
        });
    }

    println!("  Testing {} mutations ({} runs each)...", mutations.len(), config.runs_per_mutation);

    let mut mutation_results = Vec::new();

    for mutation in &mutations {
        println!("    Mutation {}: {}", mutation.id, mutation.hypothesis);

        let override_key = format!("tool.{}", target);
        let mut overrides = HashMap::new();
        overrides.insert(override_key, mutation.full_text.clone());

        let mut candidate_scores = Vec::new();
        let mut any_error = false;

        for fix in fixtures {
            match engine::run_live(fix, server, model, &overrides, config.runs_per_mutation).await {
                Ok(mut traces) => {
                    for trace in &mut traces {
                        match super::grader::grade(trace, fix, server, grader_model).await {
                            Ok(grade) => trace.grade = Some(grade),
                            Err(e) => {
                                warn!(fixture = %fix.id, mutation = %mutation.id, error = %e, "grading failed");
                            }
                        }
                    }

                    let save_dir = output_dir
                        .join(format!("round-{}", round))
                        .join(&mutation.id);
                    for trace in &traces {
                        let _ = trace.save(&save_dir);
                    }

                    candidate_scores.push(FixtureScores::from_traces(&fix.id, &traces));
                }
                Err(e) => {
                    warn!(fixture = %fix.id, mutation = %mutation.id, error = %e, "run failed");
                    any_error = true;
                    break;
                }
            }
        }

        if any_error {
            continue;
        }

        let exp_name = format!("round-{}-{}", round, mutation.id);
        let metadata = engine::build_experiment_metadata(&exp_name, &overrides.keys().cloned().collect::<Vec<_>>().iter().map(|s| format!("{}:{}", s, "(inline)")).collect::<Vec<_>>(), config.runs_per_mutation);
        let experiment =
            reporter::compute_experiment_result(metadata, baseline_scores, &candidate_scores);
        let suite_loss = compute_suite_loss(&candidate_scores, config);

        println!(
            "      Loss: {:.3} (baseline: {:.3}, delta: {:.3}) — {}",
            suite_loss,
            baseline_loss,
            suite_loss - baseline_loss,
            experiment.verdict
        );

        mutation_results.push(MutationResult {
            mutation: mutation.clone(),
            experiment,
            suite_loss,
        });
    }

    let best = mutation_results
        .iter()
        .filter(|m| !matches!(m.experiment.verdict, Verdict::Revert))
        .min_by(|a, b| a.suite_loss.partial_cmp(&b.suite_loss).unwrap());

    let (verdict, reason, best_id, best_loss) = match best {
        Some(b) if matches!(b.experiment.verdict, Verdict::Ship) => (
            Verdict::Ship,
            format!(
                "Mutation '{}' improved loss from {:.3} to {:.3}: {}",
                b.mutation.id, baseline_loss, b.suite_loss, b.mutation.hypothesis
            ),
            Some(b.mutation.id.clone()),
            Some(b.suite_loss),
        ),
        Some(b) => (
            Verdict::Inconclusive,
            format!(
                "Best mutation '{}' (loss {:.3}) not statistically significant",
                b.mutation.id, b.suite_loss
            ),
            Some(b.mutation.id.clone()),
            Some(b.suite_loss),
        ),
        None => (
            Verdict::Revert,
            "All mutations caused regressions".to_string(),
            None,
            None,
        ),
    };

    Ok(OptimizeRoundResult {
        round,
        target_component: target.to_string(),
        mutations: mutation_results,
        best_mutation: best_id,
        verdict,
        verdict_reason: reason,
        baseline_suite_loss: baseline_loss,
        best_candidate_loss: best_loss,
    })
}

pub async fn run_baseline(
    fixtures: &[Fixture],
    server: &str,
    model: Option<&str>,
    grader_model: &str,
    runs: usize,
    output_dir: &Path,
) -> Result<Vec<FixtureScores>, String> {
    println!("Running baseline ({} fixtures, {} runs each)...", fixtures.len(), runs);
    let overrides = HashMap::new();
    let mut scores = Vec::new();

    for fix in fixtures {
        println!("  Fixture: {}", fix.id);
        let mut traces = engine::run_live(fix, server, model, &overrides, runs).await?;

        for trace in &mut traces {
            match super::grader::grade(trace, fix, server, grader_model).await {
                Ok(grade) => trace.grade = Some(grade),
                Err(e) => warn!(fixture = %fix.id, error = %e, "grading failed"),
            }
        }

        let save_dir = output_dir.join("baseline");
        for trace in &traces {
            let _ = trace.save(&save_dir);
        }

        let fcsr = if !traces.is_empty() {
            let graded: Vec<_> = traces.iter().filter_map(|t| t.grade.as_ref()).collect();
            if graded.is_empty() {
                0.0
            } else {
                graded.iter().map(|g| g.first_call_success_rate).sum::<f64>() / graded.len() as f64
            }
        } else {
            0.0
        };
        println!("    FCSR: {:.0}%", fcsr * 100.0);

        scores.push(FixtureScores::from_traces(&fix.id, &traces));
    }

    Ok(scores)
}

pub fn apply_mutation(component: &str, mutation: &Mutation) -> Result<(), String> {
    let strap_dir = Path::new("crates/agent/src/strap");

    let filename = format!("{}.txt", component);
    let path = strap_dir.join(&filename);

    if !path.exists() {
        let candidates: Vec<_> = std::fs::read_dir(strap_dir)
            .map_err(|e| format!("read strap dir: {}", e))?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .map(|n| n.contains(component))
                    .unwrap_or(false)
            })
            .collect();
        if candidates.len() == 1 {
            let path = candidates[0].path();
            std::fs::write(&path, &mutation.full_text)
                .map_err(|e| format!("write {}: {}", path.display(), e))?;
            return Ok(());
        }
        return Err(format!(
            "STRAP doc not found at {} and no unique match in {}",
            path.display(),
            strap_dir.display()
        ));
    }

    std::fs::write(&path, &mutation.full_text)
        .map_err(|e| format!("write {}: {}", path.display(), e))?;

    Ok(())
}

pub fn load_config(path: &Path) -> OptimizeConfig {
    if path.exists() {
        if let Ok(contents) = std::fs::read_to_string(path) {
            if let Ok(config) = serde_yaml::from_str(&contents) {
                return config;
            }
        }
    }
    OptimizeConfig::default()
}

pub fn save_round_result(dir: &Path, result: &OptimizeRoundResult) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create dir: {}", e))?;
    let path = dir.join(format!("round-{}.json", result.round));
    let json =
        serde_json::to_string_pretty(result).map_err(|e| format!("serialize round: {}", e))?;
    std::fs::write(&path, json).map_err(|e| format!("write: {}", e))?;
    Ok(())
}

pub fn load_history(dir: &Path) -> Vec<OptimizeRoundResult> {
    let mut results = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("round-") && n.ends_with(".json"))
                .unwrap_or(false)
            {
                if let Ok(contents) = std::fs::read_to_string(&path) {
                    if let Ok(result) = serde_json::from_str(&contents) {
                        results.push(result);
                    }
                }
            }
        }
    }
    results.sort_by_key(|r: &OptimizeRoundResult| r.round);
    results
}

pub fn print_round_result(result: &OptimizeRoundResult) {
    println!();
    println!(
        "  ═══ ROUND {} — {} ═══",
        result.round, result.target_component
    );
    println!(
        "  Baseline loss: {:.3}  |  Mutations tested: {}",
        result.baseline_suite_loss,
        result.mutations.len()
    );
    println!();

    for m in &result.mutations {
        let marker = if result.best_mutation.as_deref() == Some(&m.mutation.id) {
            " ★"
        } else {
            ""
        };
        println!(
            "    {} ({}) loss={:.3} — {}{}",
            m.mutation.id, m.mutation.strategy, m.suite_loss, m.mutation.hypothesis, marker
        );
    }

    println!();
    println!("  VERDICT: {}", result.verdict);
    println!("  {}", result.verdict_reason);

    if let Some(ref best_id) = result.best_mutation {
        if let Some(best) = result.mutations.iter().find(|m| m.mutation.id == *best_id) {
            println!(
                "  Best loss: {:.3} (delta: {:.3})",
                best.suite_loss,
                best.suite_loss - result.baseline_suite_loss
            );
        }
    }
    println!();
}
