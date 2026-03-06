use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::registry::ToolResult;

/// Base input structure for STRAP tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainInput {
    #[serde(default)]
    pub resource: String,
    pub action: String,
}

/// Defines a resource and its available actions.
#[derive(Debug, Clone)]
pub struct ResourceConfig {
    pub name: String,
    pub actions: Vec<String>,
    pub description: String,
}

/// Defines a field in the domain schema.
#[derive(Debug, Clone)]
pub struct FieldConfig {
    pub name: String,
    pub field_type: String, // "string", "integer", "boolean", "array", "object"
    pub description: String,
    pub required: bool,
    pub enum_values: Vec<String>,
    pub default: Option<serde_json::Value>,
}

/// Configures JSON schema generation for domain tools.
#[derive(Debug, Clone)]
pub struct DomainSchemaConfig {
    pub domain: String,
    pub description: String,
    pub resources: HashMap<String, ResourceConfig>,
    pub fields: Vec<FieldConfig>,
    pub examples: Vec<String>,
}

/// Validate resource and action against allowed values.
pub fn validate_resource_action(
    resource: &str,
    action: &str,
    resources: &HashMap<String, ResourceConfig>,
) -> Result<(), ToolResult> {
    if resources.is_empty() {
        return Ok(());
    }

    let rc = resources.get(resource).or_else(|| resources.get(""));
    let rc = match rc {
        Some(rc) => rc,
        None => {
            let valid: Vec<&str> = resources
                .keys()
                .filter(|k| !k.is_empty())
                .map(|s| s.as_str())
                .collect();
            return Err(ToolResult::error(format!(
                "unknown resource: {} (valid: {})",
                resource,
                valid.join(", ")
            )));
        }
    };

    if rc.actions.iter().any(|a| a == action) {
        return Ok(());
    }

    Err(ToolResult::error(format!(
        "unknown action '{}' for resource '{}' (valid: {})",
        action,
        resource,
        rc.actions.join(", ")
    )))
}

/// Generate a JSON schema for a domain tool.
pub fn build_domain_schema(cfg: &DomainSchemaConfig) -> serde_json::Value {
    let mut properties = serde_json::Map::new();
    let mut required = vec![serde_json::Value::String("action".to_string())];

    // Add resource field if multiple resources
    if cfg.resources.len() > 1 {
        let resource_names: Vec<serde_json::Value> = cfg
            .resources
            .keys()
            .filter(|k| !k.is_empty())
            .map(|k| serde_json::Value::String(k.clone()))
            .collect();

        let mut resource_prop = serde_json::Map::new();
        resource_prop.insert("type".to_string(), "string".into());
        resource_prop.insert(
            "description".to_string(),
            format!(
                "Resource type: {}",
                cfg.resources
                    .keys()
                    .filter(|k| !k.is_empty())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )
            .into(),
        );
        resource_prop.insert("enum".to_string(), resource_names.into());
        properties.insert(
            "resource".to_string(),
            serde_json::Value::Object(resource_prop),
        );
        required.push(serde_json::Value::String("resource".to_string()));
    }

    // Collect all actions across all resources
    let mut action_set: Vec<String> = cfg
        .resources
        .values()
        .flat_map(|rc| rc.actions.iter().cloned())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    action_set.sort();

    let mut action_prop = serde_json::Map::new();
    action_prop.insert("type".to_string(), "string".into());
    action_prop.insert(
        "description".to_string(),
        format!("Action to perform: {}", action_set.join(", ")).into(),
    );
    action_prop.insert(
        "enum".to_string(),
        action_set
            .into_iter()
            .map(serde_json::Value::String)
            .collect::<Vec<_>>()
            .into(),
    );
    properties.insert(
        "action".to_string(),
        serde_json::Value::Object(action_prop),
    );

    // Add field definitions
    for f in &cfg.fields {
        let mut prop = serde_json::Map::new();
        prop.insert("type".to_string(), f.field_type.clone().into());
        prop.insert("description".to_string(), f.description.clone().into());

        if !f.enum_values.is_empty() {
            prop.insert(
                "enum".to_string(),
                f.enum_values
                    .iter()
                    .map(|v| serde_json::Value::String(v.clone()))
                    .collect::<Vec<_>>()
                    .into(),
            );
        }

        if let Some(ref default) = f.default {
            prop.insert("default".to_string(), default.clone());
        }

        properties.insert(f.name.clone(), serde_json::Value::Object(prop));

        if f.required {
            required.push(serde_json::Value::String(f.name.clone()));
        }
    }

    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}

/// Generate a description string for domain tools.
pub fn build_domain_description(cfg: &DomainSchemaConfig) -> String {
    let mut desc = cfg.description.clone();

    if !cfg.resources.is_empty() {
        desc.push_str("\n\nResources and Actions:");
        for (name, rc) in &cfg.resources {
            if name.is_empty() {
                continue;
            }
            desc.push_str(&format!("\n- {}: {}", name, rc.actions.join(", ")));
            if !rc.description.is_empty() {
                desc.push_str(&format!(" ({})", rc.description));
            }
        }
    }

    if !cfg.examples.is_empty() {
        desc.push_str("\n\nExamples:\n");
        for ex in &cfg.examples {
            desc.push_str(&format!("  {}\n", ex));
        }
    }

    desc
}

/// Check if an action requires user approval based on a list of dangerous actions.
pub fn action_requires_approval(action: &str, dangerous_actions: &[&str]) -> bool {
    dangerous_actions.contains(&action)
}
