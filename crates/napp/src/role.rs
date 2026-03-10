//! Role parser — ROLE.md (persona) and role.json (workflow bindings + triggers).
//!
//! ROLE.md is pure prose — the agent's job description. No frontmatter required.
//!
//! role.json carries the operational structure: which workflows run, when they
//! fire, and what dependencies the role requires.
//!
//! role.json format:
//! ```json
//! {
//!   "workflows": {
//!     "morning-briefing": {
//!       "ref": "@nebo/workflows/daily-briefing@^1.0.0",
//!       "trigger": { "type": "schedule", "cron": "0 7 * * *" }
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
}

/// A workflow bound to a role with its trigger.
///
/// The role decides *when* a workflow runs. The workflow is just the procedure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowBinding {
    /// Workflow qualified name (@org/workflows/name@version).
    #[serde(rename = "ref")]
    pub workflow_ref: String,
    /// When this workflow runs.
    pub trigger: RoleTrigger,
    /// Human-readable description of this binding.
    #[serde(default)]
    pub description: String,
    /// Default inputs passed to the workflow on trigger.
    #[serde(default)]
    pub inputs: HashMap<String, serde_json::Value>,
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
    let config: RoleConfig = serde_json::from_str(json_str)
        .map_err(|e| NappError::Manifest(format!("role.json: {}", e)))?;
    validate_role_config(&config)?;
    Ok(config)
}

/// Check if a string is a valid reference: either a qualified name with the expected
/// type segment, or an install code (e.g. WORK-XXXX-XXXX, SKIL-XXXX-XXXX).
///
/// Qualified format: `@org/type/name` or `@org/type/name@version`
/// Install code format: `PREFIX-XXXX-XXXX`
fn is_qualified_ref(s: &str, expected_type: &str) -> bool {
    // Accept install codes (WORK-XXXX-XXXX, SKIL-XXXX-XXXX, etc.)
    let code_prefix = match expected_type {
        "workflows" => "WORK-",
        "skills" => "SKIL-",
        "roles" => "ROLE-",
        _ => "",
    };
    if !code_prefix.is_empty() && s.starts_with(code_prefix) {
        return true;
    }

    // Accept qualified names
    if !s.starts_with('@') {
        return false;
    }
    // Strip the leading @ and any trailing @version
    let without_at = &s[1..];
    let name_part = if let Some(idx) = without_at.find('@') {
        &without_at[..idx]
    } else {
        without_at
    };
    // Must be org/type/name (3 segments)
    let segments: Vec<&str> = name_part.split('/').collect();
    if segments.len() != 3 {
        return false;
    }
    // Type segment must match expected
    segments[1] == expected_type
        && !segments[0].is_empty()
        && !segments[2].is_empty()
}

/// Validate role.json qualified name references and bindings.
fn validate_role_config(config: &RoleConfig) -> Result<(), NappError> {
    for (name, binding) in &config.workflows {
        // Allow empty ref for inline/ad-hoc workflows (no marketplace reference)
        if !binding.workflow_ref.is_empty() && !is_qualified_ref(&binding.workflow_ref, "workflows") {
            return Err(NappError::Manifest(format!(
                "workflow '{}' ref must be a qualified name (@org/workflows/name) or empty: {}",
                name, binding.workflow_ref
            )));
        }
        // Validate event triggers have at least one source
        if let RoleTrigger::Event { sources } = &binding.trigger {
            if sources.is_empty() {
                return Err(NappError::Manifest(format!(
                    "workflow '{}' event trigger must have at least one source",
                    name
                )));
            }
        }
    }
    for ref_str in &config.skills {
        if !is_qualified_ref(ref_str, "skills") {
            return Err(NappError::Manifest(format!(
                "skill ref must be a qualified name (@org/skills/name): {}",
                ref_str
            )));
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
    fn test_qualified_ref_validation() {
        assert!(is_qualified_ref("@acme/workflows/lead-qual@^1.0.0", "workflows"));
        assert!(is_qualified_ref("@nebo/skills/briefing-writer", "skills"));
        assert!(is_qualified_ref("WORK-ABCD-1234", "workflows"));
        assert!(!is_qualified_ref("bad-ref", "workflows"));
        assert!(!is_qualified_ref("@acme/tools/crm", "workflows")); // wrong type
        assert!(!is_qualified_ref("@/workflows/name", "workflows")); // empty org
        assert!(!is_qualified_ref("@org/workflows/", "workflows")); // empty name
    }

    #[test]
    fn test_parse_role_config() {
        let json = r#"{
            "workflows": {
                "morning-briefing": {
                    "ref": "@nebo/workflows/daily-briefing@^1.0.0",
                    "trigger": { "type": "schedule", "cron": "0 7 * * *" },
                    "description": "Daily morning briefing"
                },
                "day-monitor": {
                    "ref": "@nebo/workflows/day-monitor@^1.0.0",
                    "trigger": { "type": "heartbeat", "interval": "30m", "window": "08:00-18:00" }
                },
                "interrupt": {
                    "ref": "@nebo/workflows/urgent-interrupt@^1.0.0",
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
        assert_eq!(briefing.workflow_ref, "@nebo/workflows/daily-briefing@^1.0.0");
        assert!(matches!(briefing.trigger, RoleTrigger::Schedule { .. }));
        assert_eq!(briefing.description, "Daily morning briefing");

        let monitor = &config.workflows["day-monitor"];
        assert!(matches!(monitor.trigger, RoleTrigger::Heartbeat { .. }));

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
                    "ref": "@acme/workflows/ad-hoc@1.0.0",
                    "trigger": { "type": "manual" }
                }
            }
        }"#;
        let config = parse_role_config(json).unwrap();
        assert!(matches!(config.workflows["ad-hoc"].trigger, RoleTrigger::Manual));
    }

    #[test]
    fn test_bad_workflow_ref() {
        let json = r#"{"workflows": {"x": {"ref": "BAD-prefix", "trigger": {"type": "manual"}}}}"#;
        assert!(parse_role_config(json).is_err());
    }

    #[test]
    fn test_empty_workflow_ref_allowed() {
        let json = r#"{"workflows": {"ad-hoc": {"ref": "", "trigger": {"type": "manual"}}}}"#;
        let config = parse_role_config(json).unwrap();
        assert_eq!(config.workflows.len(), 1);
        assert!(matches!(config.workflows["ad-hoc"].trigger, RoleTrigger::Manual));
    }

    #[test]
    fn test_bad_skill_ref() {
        let json = r#"{"skills": ["BAD-prefix"]}"#;
        assert!(parse_role_config(json).is_err());
    }

    #[test]
    fn test_empty_event_sources() {
        let json = r#"{"workflows": {"x": {"ref": "@acme/workflows/x@1.0.0", "trigger": {"type": "event", "sources": []}}}}"#;
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
                    "ref": "@acme/workflows/daily-report@^1.0.0",
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
}
