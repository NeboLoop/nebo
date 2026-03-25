//! Role parser — ROLE.md (persona) and role.json (workflow bindings + triggers).
//!
//! ROLE.md is pure prose — the agent's job description. No frontmatter required.
//!
//! role.json carries the operational structure: inline workflow definitions
//! (activities, budgets, triggers) and what dependencies the role requires.
//!
//! role.json format:
//! ```json
//! {
//!   "workflows": {
//!     "morning-briefing": {
//!       "trigger": { "type": "schedule", "cron": "0 7 * * *" },
//!       "description": "Daily morning briefing",
//!       "activities": [{
//!         "id": "gather",
//!         "intent": "Gather news and calendar events"
//!       }],
//!       "budget": { "total_per_run": 5000 }
//!     }
//!   },
//!   "skills": ["@nebo/skills/briefing-writer@^1.0.0"],
//!   "pricing": { "model": "monthly_fixed", "cost": 47.0 }
//! }
//! ```

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::NappError;

// ---------------------------------------------------------------------------
// role.json — workflow bindings, triggers, dependencies, pricing
// ---------------------------------------------------------------------------

/// Role configuration parsed from role.json.
///
/// Contains the "schedule of intent" — which workflows run, when they fire,
/// and what dependencies the role requires.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleConfig {
    #[serde(default)]
    pub workflows: HashMap<String, WorkflowBinding>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub pricing: Option<RolePricing>,
    #[serde(default)]
    pub defaults: Option<RoleDefaults>,
    /// Input field definitions — rendered as a dynamic form during setup.
    /// User-supplied values are stored and injected into workflow execution.
    #[serde(default)]
    pub inputs: Vec<RoleInputField>,
}

/// A single input field the role needs from the user.
///
/// Defines the schema for a dynamic form field rendered during role setup.
/// Supported types: text, textarea, number, select, checkbox, radio.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleInputField {
    /// Unique key used to reference this value in workflows.
    /// Falls back to `name` if not provided (NeboLoop uses `name`).
    #[serde(default)]
    pub key: String,
    /// Display label shown to the user.
    /// Falls back to empty (populated from `name` in post-processing).
    #[serde(default)]
    pub label: String,
    /// NeboLoop uses `name` instead of `key` — accepted as alias.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional help text shown below the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Field type: text, textarea, number, select, checkbox, radio.
    #[serde(rename = "type", default = "default_input_type")]
    pub field_type: String,
    /// Whether the field must be filled before saving.
    #[serde(default)]
    pub required: bool,
    /// Default value (string, number, or bool depending on type).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    /// Placeholder text for text/textarea/number fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    /// Options for select/radio fields.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<RoleInputOption>,
}

/// An option in a select or radio field.
/// Accepts both `{ "value": "x", "label": "X" }` and plain `"x"` strings.
#[derive(Debug, Clone, Serialize)]
pub struct RoleInputOption {
    pub value: String,
    pub label: String,
}

impl<'de> serde::Deserialize<'de> for RoleInputOption {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de;

        struct OptionVisitor;

        impl<'de> de::Visitor<'de> for OptionVisitor {
            type Value = RoleInputOption;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a string or { value, label } object")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<RoleInputOption, E> {
                let label = v.replace('_', " ").replace('-', " ");
                Ok(RoleInputOption {
                    value: v.to_string(),
                    label,
                })
            }

            fn visit_map<M: de::MapAccess<'de>>(self, map: M) -> Result<RoleInputOption, M::Error> {
                #[derive(Deserialize)]
                struct Helper {
                    value: String,
                    #[serde(default)]
                    label: Option<String>,
                }
                let h = Helper::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(RoleInputOption {
                    label: h.label.unwrap_or_else(|| h.value.clone()),
                    value: h.value,
                })
            }
        }

        deserializer.deserialize_any(OptionVisitor)
    }
}

fn default_input_type() -> String {
    "text".to_string()
}

/// An inline workflow bound to a role with its trigger.
///
/// Activities, budget, and inputs are defined directly in role.json.
/// No external workflow references — the role owns the full procedure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowBinding {
    /// When this workflow runs.
    pub trigger: RoleTrigger,
    /// Human-readable description of this binding.
    #[serde(default)]
    pub description: String,
    /// Default inputs passed to the workflow on trigger.
    #[serde(default)]
    pub inputs: HashMap<String, serde_json::Value>,
    /// Inline activities (the procedure). Empty = chat-only binding.
    #[serde(default)]
    pub activities: Vec<RoleActivity>,
    /// Budget constraints for the entire workflow run.
    #[serde(default)]
    pub budget: RoleBudget,
    /// Event name to emit on completion (e.g. "briefing.ready").
    /// Namespaced by agent slug at runtime: "agent-name.briefing.ready".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emit: Option<String>,
}

impl WorkflowBinding {
    /// Serialize this binding into a workflow definition JSON string
    /// compatible with `workflow::parser::parse_workflow`.
    pub fn to_workflow_json(&self, name: &str) -> String {
        let inputs: HashMap<String, serde_json::Value> = self
            .inputs
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    serde_json::json!({
                        "type": "string",
                        "default": v,
                    }),
                )
            })
            .collect();

        serde_json::json!({
            "version": "1.0",
            "id": name,
            "name": name,
            "inputs": inputs,
            "activities": self.activities,
            "budget": self.budget,
            "dependencies": { "skills": [], "workflows": [] },
        })
        .to_string()
    }

    /// Returns true if this binding has inline activities to execute.
    pub fn has_activities(&self) -> bool {
        !self.activities.is_empty()
    }
}

/// A single activity in a role's inline workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleActivity {
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
    pub token_budget: RoleTokenBudget,
    #[serde(default)]
    pub on_error: RoleOnError,
}

/// Token budget for an activity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleTokenBudget {
    #[serde(default = "default_role_token_max")]
    pub max: u32,
}

fn default_role_token_max() -> u32 {
    4096
}

impl Default for RoleTokenBudget {
    fn default() -> Self {
        Self { max: default_role_token_max() }
    }
}

/// Error handling policy for an activity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleOnError {
    #[serde(default = "default_role_retry")]
    pub retry: u32,
    #[serde(default = "default_role_fallback")]
    pub fallback: RoleFallback,
}

fn default_role_retry() -> u32 { 1 }
fn default_role_fallback() -> RoleFallback { RoleFallback::NotifyOwner }

impl Default for RoleOnError {
    fn default() -> Self {
        Self {
            retry: default_role_retry(),
            fallback: default_role_fallback(),
        }
    }
}

/// Fallback strategy when an activity fails after all retries.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleFallback {
    #[serde(alias = "NotifyOwner")]
    NotifyOwner,
    #[serde(alias = "Skip")]
    Skip,
    #[serde(alias = "Abort")]
    Abort,
}

/// Budget constraints for the entire workflow run.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RoleBudget {
    #[serde(default)]
    pub total_per_run: u32,
    #[serde(default)]
    pub cost_estimate: String,
}

/// Trigger types for role-level workflow scheduling.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RoleTrigger {
    /// Cron-based, predictable schedule.
    #[serde(rename = "schedule")]
    Schedule { cron: String },
    /// Recurring interval within a time window.
    #[serde(rename = "heartbeat")]
    Heartbeat {
        interval: String,
        #[serde(default)]
        window: Option<String>,
    },
    /// Event-driven, fires when something in the world changes.
    #[serde(rename = "event")]
    Event { sources: Vec<String> },
    /// Explicit user trigger.
    #[serde(rename = "manual")]
    Manual,
}

/// Pricing configuration for a role.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolePricing {
    pub model: String,
    #[serde(default)]
    pub cost: f64,
}

/// Default settings for the role.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleDefaults {
    /// Timezone preference.
    #[serde(default)]
    pub timezone: String,
    /// JSON paths within role.json that the user can override.
    #[serde(default)]
    pub configurable: Vec<String>,
}

/// Parse a role.json file into a `RoleConfig`.
pub fn parse_role_config(json_str: &str) -> Result<RoleConfig, NappError> {
    let mut config: RoleConfig = serde_json::from_str(json_str)
        .map_err(|e| NappError::Manifest(format!("role.json: {}", e)))?;

    // Normalize input fields: NeboLoop uses `name` instead of `key`/`label`
    for field in &mut config.inputs {
        if field.key.is_empty() {
            if let Some(ref name) = field.name {
                field.key = name.clone();
            }
        }
        if field.label.is_empty() {
            let source = field.name.as_deref().unwrap_or(&field.key);
            field.label = source.replace('_', " ").replace('-', " ");
            // Capitalize first letter of each word
            field.label = field.label.split_whitespace()
                .map(|w| {
                    let mut c = w.chars();
                    match c.next() {
                        None => String::new(),
                        Some(f) => f.to_uppercase().to_string() + c.as_str(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
        }
    }

    // Normalize options: NeboLoop may send plain strings instead of {value, label} objects
    for field in &mut config.inputs {
        // Options are already typed as Vec<RoleInputOption> — if they parsed, they're fine.
        // Plain string arrays would fail serde, so they're handled by the caller.
    }

    validate_role_config(&config)?;
    Ok(config)
}

/// Check if a string is a valid skill reference: either a qualified name or install code.
fn is_qualified_skill_ref(s: &str) -> bool {
    // Accept install codes (SKIL-XXXX-XXXX)
    if s.starts_with("SKIL-") {
        return true;
    }

    // Accept qualified names (@org/skills/name or @org/skills/name@version)
    if !s.starts_with('@') {
        return false;
    }
    let without_at = &s[1..];
    let name_part = if let Some(idx) = without_at.find('@') {
        &without_at[..idx]
    } else {
        without_at
    };
    let segments: Vec<&str> = name_part.split('/').collect();
    if segments.len() != 3 {
        return false;
    }
    segments[1] == "skills" && !segments[0].is_empty() && !segments[2].is_empty()
}

/// Validate role.json bindings.
fn validate_role_config(config: &RoleConfig) -> Result<(), NappError> {
    for (name, binding) in &config.workflows {
        // Validate event triggers have at least one source
        if let RoleTrigger::Event { sources } = &binding.trigger {
            if sources.is_empty() {
                return Err(NappError::Manifest(format!(
                    "workflow '{}' event trigger must have at least one source",
                    name
                )));
            }
        }
        // Validate activity IDs are unique within a binding
        let mut seen = std::collections::HashSet::new();
        for activity in &binding.activities {
            if !activity.id.is_empty() && !seen.insert(&activity.id) {
                return Err(NappError::Manifest(format!(
                    "workflow '{}' has duplicate activity id: {}",
                    name, activity.id
                )));
            }
        }
    }
    for ref_str in &config.skills {
        if !is_qualified_skill_ref(ref_str) {
            tracing::warn!(skill_ref = %ref_str, "skill ref is not a qualified name — cascade install may skip it");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// ROLE.md — backward compat for frontmatter-based roles
// ---------------------------------------------------------------------------

/// Parsed ROLE.md frontmatter (backward compatibility).
///
/// New roles use pure prose in ROLE.md with no frontmatter. This struct
/// supports the legacy format where identity was in frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleDef {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Markdown body after the frontmatter (not from YAML).
    #[serde(skip)]
    pub body: String,
}

/// Parse a ROLE.md file.
///
/// If the file has YAML frontmatter, extracts identity fields.
/// If the file is pure prose, returns empty identity with the full content as body.
pub fn parse_role(content: &str) -> Result<RoleDef, NappError> {
    let trimmed = content.trim_start();

    // Pure prose — no frontmatter
    if !trimmed.starts_with("---") {
        return Ok(RoleDef {
            id: String::new(),
            name: String::new(),
            description: String::new(),
            body: content.trim().to_string(),
        });
    }

    // Legacy frontmatter format
    let (yaml_str, body) = split_frontmatter(content)?;
    let mut def: RoleDef =
        serde_yaml::from_str(&yaml_str).map_err(|e| NappError::Manifest(format!("role YAML: {}", e)))?;
    def.body = body;
    Ok(def)
}

/// Split `---` delimited frontmatter from the markdown body.
fn split_frontmatter(content: &str) -> Result<(String, String), NappError> {
    let trimmed = content.trim_start();
    let after_first = &trimmed[3..];
    let close_pos = after_first
        .find("\n---")
        .ok_or_else(|| NappError::Manifest("missing closing --- in frontmatter".into()))?;

    let yaml = after_first[..close_pos].trim().to_string();
    let body = after_first[close_pos + 4..].trim().to_string();

    Ok((yaml, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qualified_skill_ref_validation() {
        assert!(is_qualified_skill_ref("@nebo/skills/briefing-writer"));
        assert!(is_qualified_skill_ref("@nebo/skills/briefing-writer@^1.0.0"));
        assert!(is_qualified_skill_ref("SKIL-ABCD-1234"));
        assert!(!is_qualified_skill_ref("bad-ref"));
        assert!(!is_qualified_skill_ref("@acme/tools/crm")); // wrong type
        assert!(!is_qualified_skill_ref("@/skills/name")); // empty org
        assert!(!is_qualified_skill_ref("@org/skills/")); // empty name
    }

    #[test]
    fn test_parse_role_config_inline() {
        let json = r#"{
            "workflows": {
                "morning-briefing": {
                    "trigger": { "type": "schedule", "cron": "0 7 * * *" },
                    "description": "Daily morning briefing",
                    "activities": [{
                        "id": "gather",
                        "intent": "Gather news and calendar events",
                        "model": "sonnet",
                        "steps": ["Fetch top headlines", "Check today's calendar"]
                    }],
                    "budget": { "total_per_run": 5000 }
                },
                "day-monitor": {
                    "trigger": { "type": "heartbeat", "interval": "30m", "window": "08:00-18:00" }
                },
                "interrupt": {
                    "trigger": { "type": "event", "sources": ["calendar.changed", "email.urgent"] }
                }
            },
            "skills": ["@nebo/skills/briefing-writer@^1.0.0"],
            "pricing": { "model": "monthly_fixed", "cost": 47.0 },
            "defaults": {
                "timezone": "user_local",
                "configurable": ["workflows.morning-briefing.trigger.cron"]
            }
        }"#;
        let config = parse_role_config(json).unwrap();
        assert_eq!(config.workflows.len(), 3);

        let briefing = &config.workflows["morning-briefing"];
        assert!(matches!(briefing.trigger, RoleTrigger::Schedule { .. }));
        assert_eq!(briefing.description, "Daily morning briefing");
        assert_eq!(briefing.activities.len(), 1);
        assert_eq!(briefing.activities[0].id, "gather");
        assert_eq!(briefing.budget.total_per_run, 5000);

        let monitor = &config.workflows["day-monitor"];
        assert!(matches!(monitor.trigger, RoleTrigger::Heartbeat { .. }));
        assert!(monitor.activities.is_empty()); // chat-only binding

        let interrupt = &config.workflows["interrupt"];
        if let RoleTrigger::Event { sources } = &interrupt.trigger {
            assert_eq!(sources.len(), 2);
        } else {
            panic!("expected event trigger");
        }

        assert_eq!(config.skills, vec!["@nebo/skills/briefing-writer@^1.0.0"]);

        let pricing = config.pricing.unwrap();
        assert_eq!(pricing.model, "monthly_fixed");
        assert!((pricing.cost - 47.0).abs() < f64::EPSILON);

        let defaults = config.defaults.unwrap();
        assert_eq!(defaults.timezone, "user_local");
        assert_eq!(defaults.configurable.len(), 1);
    }

    #[test]
    fn test_manual_trigger() {
        let json = r#"{
            "workflows": {
                "ad-hoc": {
                    "trigger": { "type": "manual" }
                }
            }
        }"#;
        let config = parse_role_config(json).unwrap();
        assert!(matches!(config.workflows["ad-hoc"].trigger, RoleTrigger::Manual));
    }

    #[test]
    fn test_bad_skill_ref_warns_but_succeeds() {
        // Non-qualified skill refs warn but don't reject the config
        let json = r#"{"skills": ["BAD-prefix"]}"#;
        assert!(parse_role_config(json).is_ok());
    }

    #[test]
    fn test_empty_event_sources() {
        let json = r#"{"workflows": {"x": {"trigger": {"type": "event", "sources": []}}}}"#;
        assert!(parse_role_config(json).is_err());
    }

    #[test]
    fn test_duplicate_activity_ids() {
        let json = r#"{"workflows": {"x": {
            "trigger": {"type": "manual"},
            "activities": [
                {"id": "step1", "intent": "a"},
                {"id": "step1", "intent": "b"}
            ]
        }}}"#;
        assert!(parse_role_config(json).is_err());
    }

    #[test]
    fn test_empty_role_config() {
        let json = "{}";
        let config = parse_role_config(json).unwrap();
        assert!(config.workflows.is_empty());
        assert!(config.skills.is_empty());
        assert!(config.pricing.is_none());
    }

    #[test]
    fn test_parse_pure_prose_role_md() {
        let content = "# Chief of Staff\n\nYou manage the executive's daily rhythm.";
        let def = parse_role(content).unwrap();
        assert!(def.id.is_empty());
        assert!(def.body.contains("Chief of Staff"));
    }

    #[test]
    fn test_parse_legacy_frontmatter_role_md() {
        let content = "---\nid: sales-sdr\nname: Sales SDR\n---\n# Sales SDR\n\nBody text.";
        let def = parse_role(content).unwrap();
        assert_eq!(def.id, "sales-sdr");
        assert_eq!(def.name, "Sales SDR");
        assert!(def.body.contains("Body text."));
    }

    #[test]
    fn test_workflow_binding_with_inputs() {
        let json = r#"{
            "workflows": {
                "daily-report": {
                    "trigger": { "type": "schedule", "cron": "0 9 * * *" },
                    "inputs": { "format": "brief", "include_charts": true }
                }
            }
        }"#;
        let config = parse_role_config(json).unwrap();
        let binding = &config.workflows["daily-report"];
        assert_eq!(binding.inputs.len(), 2);
        assert_eq!(binding.inputs["format"], "brief");
    }

    #[test]
    fn test_inline_activities() {
        let json = r#"{
            "workflows": {
                "test-flow": {
                    "trigger": { "type": "manual" },
                    "activities": [{
                        "id": "step1",
                        "intent": "Do something",
                        "model": "sonnet",
                        "steps": ["Step one"]
                    }],
                    "budget": { "total_per_run": 3000 }
                }
            }
        }"#;
        let config = parse_role_config(json).unwrap();
        let binding = &config.workflows["test-flow"];
        assert_eq!(binding.activities.len(), 1);
        assert_eq!(binding.activities[0].id, "step1");
        assert_eq!(binding.activities[0].intent, "Do something");
        assert_eq!(binding.budget.total_per_run, 3000);
    }
}
