use serde::{Deserialize, Serialize};

use crate::common::ComponentId;

// ---------------------------------------------------------------------------
// Dynamic value types — the core of v0.9 data binding
// ---------------------------------------------------------------------------

/// A data binding reference to the data model via JSON Pointer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataBinding {
    /// A JSON Pointer path to a value in the data model.
    pub path: String,
}

/// Invokes a named function on the client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    /// The name of the function to call.
    pub call: String,

    /// Arguments passed to the function.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,

    /// The expected return type of the function call.
    #[serde(rename = "returnType", skip_serializing_if = "Option::is_none")]
    pub return_type: Option<FunctionReturnType>,
}

/// Possible return types for a function call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FunctionReturnType {
    String,
    Number,
    Boolean,
    Array,
    Object,
    Any,
    Void,
}

/// A string that can be a literal, a data binding, or a function call returning a string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DynamicString {
    /// A static string value.
    Literal(String),
    /// A data binding to the data model.
    Binding(DataBinding),
    /// A function call returning a string.
    Function(FunctionCall),
}

/// A number that can be literal, data-bound, or a function call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DynamicNumber {
    /// A static number value.
    Literal(f64),
    /// A data binding to the data model.
    Binding(DataBinding),
    /// A function call returning a number.
    Function(FunctionCall),
}

/// A boolean that can be literal, data-bound, or a function call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DynamicBoolean {
    /// A static boolean value.
    Literal(bool),
    /// A data binding to the data model.
    Binding(DataBinding),
    /// A function call returning a boolean.
    Function(FunctionCall),
}

/// A string list that can be literal, data-bound, or a function call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DynamicStringList {
    /// A static list of strings.
    Literal(Vec<String>),
    /// A data binding to the data model.
    Binding(DataBinding),
    /// A function call returning a string list.
    Function(FunctionCall),
}

/// A value that can be any literal, a data binding, or a function call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DynamicValue {
    /// A string literal.
    String(String),
    /// A number literal.
    Number(f64),
    /// A boolean literal.
    Bool(bool),
    /// An array literal.
    Array(Vec<serde_json::Value>),
    /// A data binding to the data model.
    Binding(DataBinding),
    /// A function call.
    Function(FunctionCall),
}

// ---------------------------------------------------------------------------
// Child list — how containers hold children
// ---------------------------------------------------------------------------

/// A template for generating children from a data model list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildListTemplate {
    /// The component to use as a template for each item.
    #[serde(rename = "componentId")]
    pub component_id: ComponentId,

    /// The path to the list in the data model.
    pub path: String,
}

/// Defines how a container holds its children.
/// Either a static array of component IDs, or a template for dynamic lists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChildList {
    /// A static list of child component IDs.
    Static(Vec<ComponentId>),
    /// A template for generating children from a data model list.
    Template(ChildListTemplate),
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/// An event to dispatch to the server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionEvent {
    /// The name of the action to dispatch.
    pub name: String,

    /// Key-value pairs for the action context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<serde_json::Value>,
}

/// Defines an interaction handler: either a server-side event or a local function call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    /// Triggers a server-side event.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<ActionEvent>,

    /// Executes a local client-side function.
    #[serde(rename = "functionCall", skip_serializing_if = "Option::is_none")]
    pub function_call: Option<FunctionCall>,
}

// ---------------------------------------------------------------------------
// Checks / Validation
// ---------------------------------------------------------------------------

/// A single validation rule applied to an input component.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckRule {
    /// The condition to evaluate (must resolve to a boolean).
    pub condition: DynamicBoolean,

    /// The error message to display if the check fails.
    pub message: String,
}

// ---------------------------------------------------------------------------
// Accessibility
// ---------------------------------------------------------------------------

/// Attributes for assistive technologies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccessibilityAttributes {
    /// A short label for the element.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<DynamicString>,

    /// Additional description for the element.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<DynamicString>,
}
