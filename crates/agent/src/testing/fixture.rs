use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// One scenario. `Serialize` exists for `nebo-cli test export`, which writes
/// a replay fixture from a real run; the `skip_serializing_if` attributes
/// keep that file to the sections a person needs to read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fixture {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target_component: String,
    /// Working directory the run executes in (`cwd` on the chat payload).
    /// None = the server's process cwd.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Agent (employee) id the run executes as (`agent_id` on the chat
    /// payload). None = the primary assistant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub setup: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub teardown: Vec<String>,
    pub conversation: Vec<ConversationTurn>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub tool_config: HashMap<String, ToolConfig>,
    #[serde(default, skip_serializing_if = "PromptAssertions::is_empty")]
    pub prompt_assertions: PromptAssertions,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub integrated_assertions: Vec<Assertion>,
    #[serde(default, skip_serializing_if = "IdealBehavior::is_empty")]
    pub ideal_behavior: IdealBehavior,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub response_quality: Vec<ResponseQualitySpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_budget: Option<ResponseBudget>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseQualitySpec {
    pub scenario: String,
    pub requirements: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseBudget {
    pub max_chars: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_lines: Option<usize>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PromptAssertions {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub first_call: Vec<Assertion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recovery: Vec<Assertion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cost: Vec<Assertion>,
}

impl PromptAssertions {
    pub fn all(&self) -> Vec<&Assertion> {
        let mut out = Vec::new();
        out.extend(self.first_call.iter());
        out.extend(self.recovery.iter());
        out.extend(self.cost.iter());
        out
    }

    fn is_empty(&self) -> bool {
        self.first_call.is_empty() && self.recovery.is_empty() && self.cost.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assertion {
    pub id: String,
    pub text: String,
    #[serde(default = "default_severity")]
    pub severity: Severity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests: Option<String>,
    /// Structured matcher evaluated deterministically from the trace, BEFORE
    /// any grader call. Present → the assertion is program-verified and never
    /// routed to the LLM judge; absent → prose-only, judged as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<Check>,
}

/// Deterministic predicate over a [`super::trace::Trace`]. All present
/// criteria must hold (AND). A check with no criteria, or arg predicates with
/// no way to select a call, is a fixture-authoring error — the run fails with
/// a diagnostic, it never falls open to the judge.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Check {
    /// 1-based tool-call ordinal to inspect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<usize>,
    /// Sugar for `call: 1`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub first_call: bool,
    /// Tool name must be one of these (single string or list in YAML).
    #[serde(default, deserialize_with = "one_or_many", skip_serializing_if = "Vec::is_empty")]
    pub tool: Vec<String>,
    /// Dot path into the call's arguments (e.g. `old_string`, `input.subject`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arg: Option<String>,
    /// Assert the arg is present (implied by equals/contains/matches).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exists: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<serde_yaml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contains: Option<String>,
    /// Regex over the arg's string form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matches: Option<String>,
    /// Exact total tool-call count for the whole trace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tool_calls: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_total_tokens: Option<usize>,
    /// Ceiling on tool results that came back as errors, whole trace.
    /// `max_errors: 0` says every call the model made was accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_errors: Option<usize>,
    /// No error result may contain any of these (case-insensitive). The way
    /// to pin a specific error shape out of a run: "is required",
    /// "not a valid install code", "timed out".
    #[serde(default, deserialize_with = "one_or_many", skip_serializing_if = "Vec::is_empty")]
    pub no_error_contains: Vec<String>,
}

fn one_or_many<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match OneOrMany::deserialize(d)? {
        OneOrMany::One(s) => vec![s],
        OneOrMany::Many(v) => v,
    })
}

fn default_severity() -> Severity {
    Severity::Important
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Critical,
    Important,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IdealBehavior {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<usize>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub narrative: String,
}

impl IdealBehavior {
    fn is_empty(&self) -> bool {
        self.tool_calls.is_none() && self.total_tokens.is_none() && self.narrative.is_empty()
    }
}

pub fn load_fixture(path: &Path) -> Result<Fixture, String> {
    let contents =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {}", path.display(), e))?;
    serde_yaml::from_str(&contents).map_err(|e| format!("parse {}: {}", path.display(), e))
}

#[derive(Debug, Clone, Deserialize)]
pub struct Suite {
    pub name: String,
    pub fixtures: Vec<String>,
}

pub fn load_suite(path: &Path) -> Result<Suite, String> {
    let contents =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {}", path.display(), e))?;
    serde_yaml::from_str(&contents).map_err(|e| format!("parse {}: {}", path.display(), e))
}
