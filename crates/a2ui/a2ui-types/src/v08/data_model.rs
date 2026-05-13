use serde::{Deserialize, Serialize};

/// A value that can be either a literal or bound to the data model.
/// Used in v0.8 for component properties like `text`, `label`, etc.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundValue {
    /// A static string value.
    #[serde(rename = "literalString", skip_serializing_if = "Option::is_none")]
    pub literal_string: Option<String>,

    /// A static number value.
    #[serde(rename = "literalNumber", skip_serializing_if = "Option::is_none")]
    pub literal_number: Option<f64>,

    /// A static boolean value.
    #[serde(rename = "literalBoolean", skip_serializing_if = "Option::is_none")]
    pub literal_boolean: Option<bool>,

    /// A static array value.
    #[serde(rename = "literalArray", skip_serializing_if = "Option::is_none")]
    pub literal_array: Option<Vec<serde_json::Value>>,

    /// A JSON Pointer path to a value in the data model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl BoundValue {
    /// Create a BoundValue from a literal string.
    pub fn from_literal_string(s: impl Into<String>) -> Self {
        Self {
            literal_string: Some(s.into()),
            literal_number: None,
            literal_boolean: None,
            literal_array: None,
            path: None,
        }
    }

    /// Create a BoundValue from a data model path.
    pub fn from_path(path: impl Into<String>) -> Self {
        Self {
            literal_string: None,
            literal_number: None,
            literal_boolean: None,
            literal_array: None,
            path: Some(path.into()),
        }
    }

    /// Create a BoundValue with both a path and a literal string (initialization shorthand).
    pub fn from_path_with_default(path: impl Into<String>, default: impl Into<String>) -> Self {
        Self {
            literal_string: Some(default.into()),
            literal_number: None,
            literal_boolean: None,
            literal_array: None,
            path: Some(path.into()),
        }
    }
}

/// Defines the children of a container component.
/// Must contain exactly one of `explicit_list` or `template`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Children {
    /// An ordered list of component IDs that are direct children.
    #[serde(rename = "explicitList", skip_serializing_if = "Option::is_none")]
    pub explicit_list: Option<Vec<String>>,

    /// A template for rendering dynamic lists of children.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<ChildTemplate>,
}

impl Children {
    /// Create children from an explicit list of component IDs.
    pub fn from_explicit_list(ids: Vec<String>) -> Self {
        Self {
            explicit_list: Some(ids),
            template: None,
        }
    }

    /// Create children from a template.
    pub fn from_template(template: ChildTemplate) -> Self {
        Self {
            explicit_list: None,
            template: Some(template),
        }
    }
}

/// A template for generating dynamic lists of children from a data-bound list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildTemplate {
    /// The path to a list in the data model.
    #[serde(rename = "dataBinding")]
    pub data_binding: String,

    /// The ID of the component to use as a template for each item.
    #[serde(rename = "componentId")]
    pub component_id: String,
}

/// A single data entry in a `dataModelUpdate` message.
/// Each entry has a key and exactly one typed value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataEntry {
    /// The key for this data entry.
    pub key: String,

    /// A string value.
    #[serde(rename = "valueString", skip_serializing_if = "Option::is_none")]
    pub value_string: Option<String>,

    /// A number value.
    #[serde(rename = "valueNumber", skip_serializing_if = "Option::is_none")]
    pub value_number: Option<f64>,

    /// A boolean value.
    #[serde(rename = "valueBoolean", skip_serializing_if = "Option::is_none")]
    pub value_boolean: Option<bool>,

    /// A map value, represented as a nested adjacency list.
    #[serde(rename = "valueMap", skip_serializing_if = "Option::is_none")]
    pub value_map: Option<Vec<DataEntry>>,
}

/// An action definition on an interactive component (e.g. Button).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    /// The name of the action to dispatch to the server.
    pub name: String,

    /// Key-value pairs for the action context. Values can be literals or data-bound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<Vec<ActionContextEntry>>,
}

/// A single entry in an action's context array.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionContextEntry {
    /// The key name.
    pub key: String,

    /// The value, which can be literal or data-bound.
    pub value: BoundValue,
}
