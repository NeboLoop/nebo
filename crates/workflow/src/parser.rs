use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use crate::WorkflowError;

/// Top-level workflow definition (parsed from workflow.json).
///
/// Triggers are no longer part of workflow.json — they are owned by Roles
/// (via role.json). Legacy `triggers` fields are silently ignored on parse.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDef {
    pub version: String,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub inputs: HashMap<String, InputParam>,
    pub activities: Vec<Activity>,
    #[serde(default)]
    pub dependencies: Dependencies,
    #[serde(default)]
    pub budget: Budget,
}

/// Input parameter definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputParam {
    #[serde(rename = "type")]
    pub param_type: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
}

/// A single activity in the workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Activity {
    pub id: String,
    pub intent: String,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub mcps: Vec<String>,
    #[serde(default)]
    pub cmds: Vec<String>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub steps: Vec<String>,
    #[serde(default)]
    pub token_budget: TokenBudget,
    #[serde(default)]
    pub on_error: OnError,
}

/// Token budget for an activity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBudget {
    #[serde(default = "default_token_max")]
    pub max: u32,
}

fn default_token_max() -> u32 {
    4096
}

impl Default for TokenBudget {
    fn default() -> Self {
        Self { max: default_token_max() }
    }
}

/// Error handling policy for an activity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnError {
    #[serde(default = "default_retry")]
    pub retry: u32,
    #[serde(default = "default_fallback")]
    pub fallback: Fallback,
}

fn default_retry() -> u32 { 1 }
fn default_fallback() -> Fallback { Fallback::NotifyOwner }

impl Default for OnError {
    fn default() -> Self {
        Self {
            retry: default_retry(),
            fallback: default_fallback(),
        }
    }
}

/// Fallback strategy when an activity fails after all retries.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fallback {
    NotifyOwner,
    Skip,
    Abort,
}

/// Workflow dependencies — qualified names that must be installed.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Dependencies {
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub workflows: Vec<String>,
}

/// Budget constraints for the entire workflow run.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Budget {
    #[serde(default)]
    pub total_per_run: u32,
    #[serde(default)]
    pub cost_estimate: String,
}

/// Parse a workflow definition from JSON.
pub fn parse_workflow(json_str: &str) -> Result<WorkflowDef, WorkflowError> {
    let def: WorkflowDef = serde_json::from_str(json_str)
        .map_err(|e| WorkflowError::Parse(e.to_string()))?;
    validate_workflow(&def)?;
    Ok(def)
}

/// Validate a parsed workflow definition.
pub fn validate_workflow(def: &WorkflowDef) -> Result<(), WorkflowError> {
    if def.id.is_empty() {
        return Err(WorkflowError::Validation("workflow id is required".into()));
    }
    if def.name.is_empty() {
        return Err(WorkflowError::Validation("workflow name is required".into()));
    }
    if def.activities.is_empty() {
        return Err(WorkflowError::Validation("at least one activity is required".into()));
    }

    // Check activity IDs are unique
    let mut seen = std::collections::HashSet::new();
    for activity in &def.activities {
        if activity.id.is_empty() {
            return Err(WorkflowError::Validation("activity id is required".into()));
        }
        if !seen.insert(&activity.id) {
            return Err(WorkflowError::Validation(format!(
                "duplicate activity id: {}",
                activity.id
            )));
        }
    }

    // Validate budget sums if total_per_run is set
    if def.budget.total_per_run > 0 {
        let sum: u32 = def.activities.iter().map(|a| a.token_budget.max).sum();
        if sum > def.budget.total_per_run {
            return Err(WorkflowError::Validation(format!(
                "activity token budgets sum ({}) exceeds total_per_run ({})",
                sum, def.budget.total_per_run
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic_workflow() {
        let json = r#"{
            "version": "1.0",
            "id": "test-wf",
            "name": "Test Workflow",
            "inputs": {},
            "activities": [{
                "id": "step1",
                "intent": "Do something",
                "model": "sonnet",
                "steps": ["Step one"],
                "token_budget": {"max": 1000}
            }],
            "dependencies": {"skills": []},
            "budget": {"total_per_run": 1000, "cost_estimate": "$0.001"}
        }"#;
        let def = parse_workflow(json).unwrap();
        assert_eq!(def.id, "test-wf");
        assert_eq!(def.activities.len(), 1);
    }

    #[test]
    fn test_parse_legacy_triggers_ignored() {
        // Legacy workflow.json with triggers should parse without error
        let json = r#"{
            "version": "1.0",
            "id": "test-wf",
            "name": "Test",
            "triggers": [{"type": "manual"}],
            "activities": [{
                "id": "step1",
                "intent": "Do something"
            }]
        }"#;
        let def = parse_workflow(json).unwrap();
        assert_eq!(def.id, "test-wf");
    }

    #[test]
    fn test_validate_duplicate_activity_id() {
        let json = r#"{
            "version": "1.0",
            "id": "test",
            "name": "Test",
            "activities": [
                {"id": "step1", "intent": "a", "steps": []},
                {"id": "step1", "intent": "b", "steps": []}
            ]
        }"#;
        assert!(parse_workflow(json).is_err());
    }

}
