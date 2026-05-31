use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trace {
    pub fixture_id: String,
    pub run_id: String,
    pub model: String,
    pub timestamp: String,
    #[serde(default)]
    pub overrides: Vec<String>,
    pub tool_calls: Vec<TracedToolCall>,
    pub final_response: TracedResponse,
    pub metrics: TraceMetrics,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grade: Option<GradeResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracedToolCall {
    pub sequence: usize,
    pub tool: String,
    pub arguments: serde_json::Value,
    pub response: TracedToolResponse,
    pub latency_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracedToolResponse {
    pub content: String,
    pub is_error: bool,
    pub char_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TracedResponse {
    pub content: String,
    pub tokens: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TraceMetrics {
    pub total_tool_calls: usize,
    pub total_tokens: usize,
    pub input_tokens: usize,
    pub output_tokens: usize,
    pub total_latency_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GradeResult {
    pub assertions: Vec<AssertionResult>,
    pub first_call_success_rate: f64,
    pub context_pollution_score: f64,
    #[serde(default)]
    pub tool_quality: Vec<ToolQualityScore>,
    #[serde(default)]
    pub model_behavior: Vec<ModelBehaviorScore>,
    #[serde(default)]
    pub overall_notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssertionResult {
    pub id: String,
    pub passed: bool,
    #[serde(default)]
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolQualityScore {
    pub tool_call_sequence: usize,
    pub tool: String,
    pub response_parseable: bool,
    pub response_human_readable: bool,
    pub response_actionable: bool,
    pub response_within_budget: bool,
    pub score: f64,
    #[serde(default)]
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelBehaviorScore {
    pub tool_call_sequence: usize,
    pub correct_tool: bool,
    pub correct_args: bool,
    pub correct_interpretation: bool,
    pub unnecessary_retry: bool,
    pub score: f64,
    #[serde(default)]
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentMetadata {
    pub name: String,
    pub timestamp: String,
    pub git_commit: String,
    pub git_branch: String,
    pub strap_doc_hashes: HashMap<String, String>,
    pub overrides: Vec<String>,
    pub runs_per_fixture: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixtureScores {
    pub fixture_id: String,
    pub fcsr_values: Vec<f64>,
    pub pollution_values: Vec<f64>,
    pub token_counts: Vec<usize>,
    pub tool_call_counts: Vec<usize>,
    pub assertion_pass_rates: HashMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentComparison {
    pub fixture_id: String,
    pub baseline_fcsr: StatSummary,
    pub candidate_fcsr: StatSummary,
    pub fcsr_delta: f64,
    pub fcsr_p_value: f64,
    pub significant: bool,
    pub regressed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatSummary {
    pub mean: f64,
    pub std_dev: f64,
    pub n: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Verdict {
    Ship,
    Revert,
    Inconclusive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentResult {
    pub metadata: ExperimentMetadata,
    pub fixture_scores: Vec<FixtureScores>,
    pub comparisons: Vec<ExperimentComparison>,
    pub verdict: Verdict,
    pub verdict_reason: String,
}

impl FixtureScores {
    pub fn from_traces(fixture_id: &str, traces: &[Trace]) -> Self {
        let graded: Vec<_> = traces.iter().filter_map(|t| t.grade.as_ref()).collect();

        let fcsr_values: Vec<f64> = graded.iter().map(|g| g.first_call_success_rate).collect();
        let pollution_values: Vec<f64> = graded.iter().map(|g| g.context_pollution_score).collect();
        let token_counts: Vec<usize> = traces.iter().map(|t| t.metrics.total_tokens).collect();
        let tool_call_counts: Vec<usize> = traces.iter().map(|t| t.metrics.total_tool_calls).collect();

        let mut assertion_pass_rates: HashMap<String, f64> = HashMap::new();
        if let Some(first) = graded.first() {
            for assertion in &first.assertions {
                let pass_count = graded.iter().filter(|g| {
                    g.assertions.iter().any(|a| a.id == assertion.id && a.passed)
                }).count();
                let rate = if graded.is_empty() { 0.0 } else { pass_count as f64 / graded.len() as f64 };
                assertion_pass_rates.insert(assertion.id.clone(), rate);
            }
        }

        Self {
            fixture_id: fixture_id.to_string(),
            fcsr_values,
            pollution_values,
            token_counts,
            tool_call_counts,
            assertion_pass_rates,
        }
    }
}

impl StatSummary {
    pub fn from_values(values: &[f64]) -> Self {
        let n = values.len();
        if n == 0 {
            return Self { mean: 0.0, std_dev: 0.0, n: 0 };
        }
        let mean = values.iter().sum::<f64>() / n as f64;
        let variance = if n > 1 {
            values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1) as f64
        } else {
            0.0
        };
        Self { mean, std_dev: variance.sqrt(), n }
    }
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Verdict::Ship => write!(f, "SHIP"),
            Verdict::Revert => write!(f, "REVERT"),
            Verdict::Inconclusive => write!(f, "INCONCLUSIVE"),
        }
    }
}

pub fn compute_strap_hashes() -> HashMap<String, String> {
    let docs = [
        "system", "web", "bot", "loop", "message", "event", "app",
        "desktop", "organizer", "skill", "agent", "vm", "publisher", "emit",
    ];
    let mut hashes = HashMap::new();
    for name in &docs {
        if let Some(content) = crate::prompt::strap_tool_doc(name) {
            let mut hasher = Sha256::new();
            hasher.update(content.as_bytes());
            let hash = format!("{:x}", hasher.finalize());
            hashes.insert(name.to_string(), hash[..12].to_string());
        }
    }
    hashes
}

impl Trace {
    pub fn save(&self, dir: &std::path::Path) -> Result<(), String> {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("create dir {}: {}", dir.display(), e))?;
        let filename = format!("{}_{}.json", self.fixture_id, self.run_id);
        let path = dir.join(&filename);
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("serialize trace: {}", e))?;
        std::fs::write(&path, json)
            .map_err(|e| format!("write {}: {}", path.display(), e))?;
        Ok(())
    }

    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| format!("read {}: {}", path.display(), e))?;
        serde_json::from_str(&contents)
            .map_err(|e| format!("parse {}: {}", path.display(), e))
    }

    pub fn load_dir(dir: &std::path::Path) -> Result<Vec<Self>, String> {
        let mut traces = Vec::new();
        let entries = std::fs::read_dir(dir)
            .map_err(|e| format!("read dir {}: {}", dir.display(), e))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("dir entry: {}", e))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                traces.push(Self::load(&path)?);
            }
        }
        traces.sort_by(|a, b| a.run_id.cmp(&b.run_id));
        Ok(traces)
    }
}
