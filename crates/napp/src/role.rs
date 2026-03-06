//! ROLE.md parser — extracts YAML frontmatter and markdown body from role files.
//!
//! Format:
//! ```text
//! ---
//! id: sales-sdr
//! name: Sales SDR
//! description: Outbound sales development representative
//! workflows:
//!   - WORK-lead-qualification
//! tools:
//!   - TOOL-crm-lookup
//! skills:
//!   - SKILL-sales-qualification
//! pricing:
//!   model: monthly_fixed
//!   cost: 47.0
//! ---
//! # Sales SDR Role
//! Markdown body describing behavior...
//! ```

use serde::{Deserialize, Serialize};

use crate::NappError;

/// Parsed ROLE.md frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub workflows: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub pricing: Option<RolePricing>,
    /// Markdown body after the frontmatter (not from YAML).
    #[serde(skip)]
    pub body: String,
}

/// Pricing configuration for a role.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolePricing {
    pub model: String, // "monthly_fixed", "per_turn", etc.
    #[serde(default)]
    pub cost: f64,
}

/// Parse a ROLE.md file into a `RoleDef`.
///
/// The file must have YAML frontmatter delimited by `---` lines.
pub fn parse_role(content: &str) -> Result<RoleDef, NappError> {
    let (yaml_str, body) = split_frontmatter(content)?;
    let mut def: RoleDef =
        serde_yaml::from_str(&yaml_str).map_err(|e| NappError::Manifest(format!("role YAML: {}", e)))?;
    def.body = body;
    validate(&def)?;
    Ok(def)
}

/// Split `---` delimited frontmatter from the markdown body.
fn split_frontmatter(content: &str) -> Result<(String, String), NappError> {
    let trimmed = content.trim_start();

    if !trimmed.starts_with("---") {
        return Err(NappError::Manifest(
            "ROLE.md must start with YAML frontmatter (---)".into(),
        ));
    }

    // Find the closing ---
    let after_first = &trimmed[3..];
    let close_pos = after_first
        .find("\n---")
        .ok_or_else(|| NappError::Manifest("missing closing --- in frontmatter".into()))?;

    let yaml = after_first[..close_pos].trim().to_string();
    let body = after_first[close_pos + 4..].trim().to_string();

    Ok((yaml, body))
}

/// Validate required fields and code prefixes.
fn validate(def: &RoleDef) -> Result<(), NappError> {
    if def.id.is_empty() {
        return Err(NappError::Manifest("role id is required".into()));
    }
    if def.name.is_empty() {
        return Err(NappError::Manifest("role name is required".into()));
    }

    for code in &def.skills {
        if !code.starts_with("SKILL-") {
            return Err(NappError::Manifest(format!(
                "skill code must start with SKILL-: {}",
                code
            )));
        }
    }
    for code in &def.tools {
        if !code.starts_with("TOOL-") {
            return Err(NappError::Manifest(format!(
                "tool code must start with TOOL-: {}",
                code
            )));
        }
    }
    for code in &def.workflows {
        if !code.starts_with("WORK-") {
            return Err(NappError::Manifest(format!(
                "workflow code must start with WORK-: {}",
                code
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_role() {
        let content = r#"---
id: sales-sdr
name: Sales SDR
description: Outbound sales development representative
workflows:
  - WORK-lead-qualification
tools:
  - TOOL-crm-lookup
skills:
  - SKILL-sales-qualification
pricing:
  model: monthly_fixed
  cost: 47.0
---
# Sales SDR Role

This role handles outbound sales development.
"#;
        let def = parse_role(content).unwrap();
        assert_eq!(def.id, "sales-sdr");
        assert_eq!(def.name, "Sales SDR");
        assert_eq!(def.workflows, vec!["WORK-lead-qualification"]);
        assert_eq!(def.tools, vec!["TOOL-crm-lookup"]);
        assert_eq!(def.skills, vec!["SKILL-sales-qualification"]);
        assert!(def.body.contains("# Sales SDR Role"));

        let pricing = def.pricing.unwrap();
        assert_eq!(pricing.model, "monthly_fixed");
        assert!((pricing.cost - 47.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_missing_frontmatter() {
        let content = "# Just markdown\nNo frontmatter here.";
        assert!(parse_role(content).is_err());
    }

    #[test]
    fn test_missing_id() {
        let content = "---\nname: Test\n---\nBody";
        assert!(parse_role(content).is_err());
    }

    #[test]
    fn test_bad_skill_prefix() {
        let content = "---\nid: test\nname: Test\nskills:\n  - BAD-prefix\n---\nBody";
        assert!(parse_role(content).is_err());
    }

    #[test]
    fn test_minimal_role() {
        let content = "---\nid: minimal\nname: Minimal Role\n---\nBody text.";
        let def = parse_role(content).unwrap();
        assert_eq!(def.id, "minimal");
        assert!(def.workflows.is_empty());
        assert!(def.pricing.is_none());
        assert_eq!(def.body, "Body text.");
    }
}
