use serde::{Deserialize, Serialize};

use super::common_types::FunctionReturnType;

/// A collection of component and function definitions that make up a catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    /// Unique identifier for this catalog.
    #[serde(rename = "catalogId")]
    pub catalog_id: String,

    /// Definitions for UI components supported by this catalog.
    /// Each key is a component type name, each value is its JSON Schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub components: Option<serde_json::Value>,

    /// Definitions for functions supported by this catalog.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub functions: Option<Vec<FunctionDefinition>>,

    /// Theme schema — each key is a theme property name, each value is its JSON Schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<serde_json::Value>,
}

/// Describes a function's interface within a catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionDefinition {
    /// The unique name of the function.
    pub name: String,

    /// A human-readable description of what the function does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// A JSON Schema describing the expected arguments.
    pub parameters: serde_json::Value,

    /// The type of value this function returns.
    #[serde(rename = "returnType")]
    pub return_type: FunctionReturnType,
}
