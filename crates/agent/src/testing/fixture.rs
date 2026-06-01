use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct Fixture {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub target_component: String,
    #[serde(default)]
    pub setup: Vec<String>,
    #[serde(default)]
    pub teardown: Vec<String>,
    pub conversation: Vec<ConversationTurn>,
    #[serde(default)]
    pub tool_config: HashMap<String, ToolConfig>,
    #[serde(default)]
    pub prompt_assertions: PromptAssertions,
    #[serde(default)]
    pub integrated_assertions: Vec<Assertion>,
    #[serde(default)]
    pub ideal_behavior: IdealBehavior,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConversationTurn {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ToolConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub response_quality: Vec<ResponseQualitySpec>,
    #[serde(default)]
    pub response_budget: Option<ResponseBudget>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResponseQualitySpec {
    pub scenario: String,
    pub requirements: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResponseBudget {
    pub max_chars: usize,
    #[serde(default)]
    pub max_lines: Option<usize>,
    #[serde(default)]
    pub rationale: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PromptAssertions {
    #[serde(default)]
    pub first_call: Vec<Assertion>,
    #[serde(default)]
    pub recovery: Vec<Assertion>,
    #[serde(default)]
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
}

#[derive(Debug, Clone, Deserialize)]
pub struct Assertion {
    pub id: String,
    pub text: String,
    #[serde(default = "default_severity")]
    pub severity: Severity,
    #[serde(default)]
    pub metric: Option<String>,
    #[serde(default)]
    pub threshold: Option<f64>,
    #[serde(default)]
    pub tests: Option<String>,
}

fn default_severity() -> Severity {
    Severity::Important
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Critical,
    Important,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct IdealBehavior {
    #[serde(default)]
    pub tool_calls: Option<usize>,
    #[serde(default)]
    pub total_tokens: Option<usize>,
    #[serde(default)]
    pub narrative: String,
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
