use serde::{Deserialize, Serialize};

/// A structured validation error matching the A2UI `VALIDATION_FAILED` format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidationError {
    /// Error code — typically `"VALIDATION_FAILED"`.
    pub code: String,

    /// The ID of the surface where the error occurred.
    #[serde(rename = "surfaceId")]
    pub surface_id: String,

    /// The JSON pointer to the field that failed validation.
    pub path: String,

    /// A short description of why validation failed.
    pub message: String,
}

/// A reusable JSON Schema validator that holds a compiled schema.
pub struct SchemaValidator {
    schema: serde_json::Value,
}

impl SchemaValidator {
    /// Create a new validator from a JSON Schema value.
    pub fn new(schema: serde_json::Value) -> Self {
        Self { schema }
    }

    /// Validate a JSON value against the compiled schema.
    ///
    /// Returns `Ok(())` if the value is valid, or a list of `ValidationError`s.
    pub fn validate(
        &self,
        instance: &serde_json::Value,
        surface_id: &str,
    ) -> Result<(), Vec<ValidationError>> {
        validate(&self.schema, instance, surface_id)
    }
}

/// Validate a JSON value against a JSON Schema.
///
/// This is a standalone function for one-off validation without holding a compiled schema.
///
/// # Arguments
/// * `schema` - The JSON Schema to validate against.
/// * `instance` - The JSON value to validate.
/// * `surface_id` - The surface ID to include in any validation errors.
///
/// # Returns
/// `Ok(())` if valid, or `Err(Vec<ValidationError>)` with all validation failures.
pub fn validate(
    schema: &serde_json::Value,
    instance: &serde_json::Value,
    surface_id: &str,
) -> Result<(), Vec<ValidationError>> {
    let validator = match jsonschema::validator_for(schema) {
        Ok(v) => v,
        Err(e) => {
            return Err(vec![ValidationError {
                code: "SCHEMA_COMPILATION_ERROR".to_string(),
                surface_id: surface_id.to_string(),
                path: "/".to_string(),
                message: format!("Failed to compile JSON schema: {e}"),
            }]);
        }
    };

    let validation_errors: Vec<ValidationError> = validator
        .iter_errors(instance)
        .map(|e| {
            let path = format!("{}", e.instance_path);
            ValidationError {
                code: "VALIDATION_FAILED".to_string(),
                surface_id: surface_id.to_string(),
                path,
                message: e.to_string(),
            }
        })
        .collect();

    if validation_errors.is_empty() {
        Ok(())
    } else {
        Err(validation_errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_valid_instance() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            },
            "required": ["name"]
        });
        let instance = json!({ "name": "hello" });
        assert!(validate(&schema, &instance, "test-surface").is_ok());
    }

    #[test]
    fn test_invalid_instance_missing_required() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            },
            "required": ["name"]
        });
        let instance = json!({});
        let result = validate(&schema, &instance, "test-surface");
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(!errors.is_empty());
        assert_eq!(errors[0].code, "VALIDATION_FAILED");
        assert_eq!(errors[0].surface_id, "test-surface");
    }

    #[test]
    fn test_invalid_instance_wrong_type() {
        let schema = json!({
            "type": "object",
            "properties": {
                "age": { "type": "number" }
            }
        });
        let instance = json!({ "age": "not a number" });
        let result = validate(&schema, &instance, "s1");
        assert!(result.is_err());
    }

    #[test]
    fn test_schema_validator_reuse() {
        let schema = json!({
            "type": "object",
            "properties": {
                "x": { "type": "integer" }
            },
            "required": ["x"]
        });
        let validator = SchemaValidator::new(schema);
        assert!(validator.validate(&json!({"x": 1}), "s1").is_ok());
        assert!(validator.validate(&json!({}), "s1").is_err());
    }
}
