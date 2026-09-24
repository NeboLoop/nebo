//! Validation of a call's input against its tool's JSON Schema, before the
//! tool's own checks and before permission. The issues read the way the
//! model needs them: which parameter, and what was expected.

use jsonschema::error::{TypeKind, ValidationErrorKind};

/// A tool's compiled schema. `None` when the schema doesn't compile; the
/// registry logs that and the call runs unvalidated.
pub fn compile(tool: &str, schema: &serde_json::Value) -> Option<jsonschema::Validator> {
    match jsonschema::validator_for(schema) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!(tool, error = %e, "tool schema does not compile; its calls are not validated");
            None
        }
    }
}

/// Every way `input` fails the schema, one line each.
pub fn issues(validator: &jsonschema::Validator, input: &serde_json::Value) -> Vec<String> {
    validator
        .iter_errors(input)
        .map(|e| {
            let param = param_name(&e.instance_path.to_string());
            match &e.kind {
                ValidationErrorKind::Required { property } => format!(
                    "The required parameter `{}` is missing",
                    property.as_str().unwrap_or_default()
                ),
                ValidationErrorKind::Type {
                    kind: TypeKind::Single(expected),
                } if !param.is_empty() => format!(
                    "The parameter `{param}` type is expected as `{expected}` but provided as `{}`",
                    json_type(&e.instance)
                ),
                _ if param.is_empty() => e.to_string(),
                _ => format!("The parameter `{param}` is invalid: {e}"),
            }
        })
        .collect()
}

/// The smallest call the schema accepts, for a call that sent nothing:
/// its required parameters with their types as placeholders.
pub fn minimal_call(schema: &serde_json::Value) -> Option<String> {
    let required = schema.get("required")?.as_array()?;
    if required.is_empty() {
        return None;
    }
    let props = schema.get("properties");
    let fields: Vec<String> = required
        .iter()
        .filter_map(|r| r.as_str())
        .map(|name| {
            let ty = props
                .and_then(|p| p.get(name))
                .and_then(|p| p.get("type"))
                .and_then(|t| t.as_str())
                .unwrap_or("value");
            format!("\"{name}\": <{ty}>")
        })
        .collect();
    Some(format!("A minimal valid call: {{{}}}", fields.join(", ")))
}

/// `/a/0/b` → `a.0.b`.
fn param_name(pointer: &str) -> String {
    pointer.trim_start_matches('/').replace('/', ".")
}

fn json_type(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "limit": {"type": "integer"},
                "mode": {"type": "string", "enum": ["a", "b"]}
            },
            "required": ["path"]
        })
    }

    #[test]
    fn issues_name_the_parameter_and_what_was_expected() {
        let v = compile("t", &schema()).unwrap();
        assert!(issues(&v, &json!({"path": "/x", "limit": 5})).is_empty());
        assert_eq!(
            issues(&v, &json!({"limit": 5})),
            vec!["The required parameter `path` is missing"]
        );
        assert_eq!(
            issues(&v, &json!({"path": 3})),
            vec!["The parameter `path` type is expected as `string` but provided as `integer`"]
        );
        let bad_enum = issues(&v, &json!({"path": "/x", "mode": "c"}));
        assert_eq!(bad_enum.len(), 1);
        assert!(bad_enum[0].starts_with("The parameter `mode` is invalid:"), "{bad_enum:?}");
    }

    #[test]
    fn an_empty_call_gets_the_minimal_shape() {
        assert_eq!(
            minimal_call(&schema()).as_deref(),
            Some("A minimal valid call: {\"path\": <string>}")
        );
        assert_eq!(minimal_call(&json!({"type": "object"})), None);
    }
}
