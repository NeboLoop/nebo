use a2ui_types::common::SpecVersion;
use a2ui_types::v09::catalog::Catalog;

/// Context to inject into an LLM system prompt so that it can generate valid A2UI messages.
#[derive(Debug, Clone)]
pub struct PromptContext {
    /// The spec version being used.
    pub spec_version: SpecVersion,

    /// The catalog schema as a JSON string (for the LLM to reference).
    pub catalog_schema_json: String,

    /// Protocol description and rules text for the LLM.
    pub protocol_description: String,

    /// Example messages demonstrating valid A2UI output.
    pub examples: String,
}

impl std::fmt::Display for PromptContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}\n\n## Catalog Schema\n\n```json\n{}\n```\n\n## Examples\n\n{}",
            self.protocol_description, self.catalog_schema_json, self.examples
        )
    }
}

/// Build a prompt context for a v0.9 catalog.
///
/// Generates a `PromptContext` containing the protocol description, catalog schema,
/// and example messages that can be injected into an LLM system prompt.
///
/// # Arguments
/// * `catalog` — The catalog to build the prompt context for.
/// * `catalog_schema` — The raw JSON Schema of the catalog (for inclusion in prompt).
pub fn build_prompt_context_v09(
    catalog: &Catalog,
    catalog_schema: &serde_json::Value,
) -> PromptContext {
    let catalog_schema_json =
        serde_json::to_string_pretty(catalog_schema).unwrap_or_else(|_| "{}".to_string());

    let protocol_description = format!(
        r#"You are generating UI using the A2UI v0.9 protocol. A2UI is a JSON-based streaming protocol for agent-driven user interfaces.

## Protocol Rules

1. Every message must be a JSON object with a "version" field set to "v0.9".
2. Each message must contain exactly ONE of: "createSurface", "updateComponents", "updateDataModel", or "deleteSurface".
3. A surface must be created with "createSurface" before sending "updateComponents" or "updateDataModel" to it.
4. Components use a flat adjacency list model — each component has a unique "id" and references children by their IDs.
5. One component must have id "root" to serve as the root of the component tree.
6. The "component" field specifies the component type (e.g., "Text", "Button", "Column").
7. Component properties are defined inline alongside "id" and "component".
8. Data bindings use JSON Pointer paths (e.g., "/user/name") via the "path" property in a binding object.
9. You must only use components and functions defined in the catalog below.

## Catalog: {catalog_id}"#,
        catalog_id = catalog.catalog_id
    );

    let examples = r#"### Example: Create a surface and add components

```json
{"version": "v0.9", "createSurface": {"surfaceId": "my_surface", "catalogId": "CATALOG_ID_HERE"}}
```

```json
{"version": "v0.9", "updateComponents": {"surfaceId": "my_surface", "components": [{"id": "root", "component": "Column", "children": ["greeting"]}, {"id": "greeting", "component": "Text", "text": "Hello, world!"}]}}
```

### Example: Update the data model

```json
{"version": "v0.9", "updateDataModel": {"surfaceId": "my_surface", "path": "/user/name", "value": "Alice"}}
```

### Example: Delete a surface

```json
{"version": "v0.9", "deleteSurface": {"surfaceId": "my_surface"}}
```
"#
    .to_string();

    PromptContext {
        spec_version: SpecVersion::V0_9,
        catalog_schema_json,
        protocol_description,
        examples,
    }
}

/// Build a prompt context for a v0.8 catalog.
///
/// Generates a `PromptContext` containing the protocol description, catalog schema,
/// and example messages that can be injected into an LLM system prompt.
///
/// # Arguments
/// * `catalog_id` — The catalog ID being used.
/// * `catalog_schema` — The raw JSON Schema of the catalog.
pub fn build_prompt_context_v08(
    catalog_id: &str,
    catalog_schema: &serde_json::Value,
) -> PromptContext {
    let catalog_schema_json =
        serde_json::to_string_pretty(catalog_schema).unwrap_or_else(|_| "{}".to_string());

    let protocol_description = format!(
        r#"You are generating UI using the A2UI v0.8 protocol. A2UI is a JSONL-based streaming protocol for agent-driven user interfaces.

## Protocol Rules

1. Each line in the JSONL stream is a separate JSON object (a single A2UI message).
2. Each message must contain exactly ONE of: "surfaceUpdate", "dataModelUpdate", "beginRendering", or "deleteSurface".
3. Components are sent via "surfaceUpdate" messages and use a flat adjacency list model.
4. Each component has a unique "id" and a "component" object with exactly one key (the component type).
5. "beginRendering" signals the client to start rendering, specifying the root component ID.
6. "dataModelUpdate" uses typed entries (valueString, valueNumber, valueBoolean, valueMap).
7. Data bindings use BoundValue objects with "literalString"/"path" etc.
8. You must only use components defined in the catalog below.

## Catalog: {catalog_id}"#
    );

    let examples = r#"### Example: Surface update with components

```jsonl
{"surfaceUpdate": {"components": [{"id": "root", "component": {"Column": {"children": {"explicitList": ["greeting"]}}}}]}}
{"surfaceUpdate": {"components": [{"id": "greeting", "component": {"Text": {"text": {"literalString": "Hello, world!"}}}}]}}
```

### Example: Data model update

```jsonl
{"dataModelUpdate": {"surfaceId": "main", "contents": [{"key": "name", "valueString": "Alice"}]}}
```

### Example: Begin rendering

```jsonl
{"beginRendering": {"surfaceId": "main", "root": "root"}}
```
"#
    .to_string();

    PromptContext {
        spec_version: SpecVersion::V0_8,
        catalog_schema_json,
        protocol_description,
        examples,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_build_prompt_context_v09() {
        let catalog = Catalog {
            catalog_id: "https://example.com/basic.json".to_string(),
            components: Some(json!({"Text": {"type": "object"}})),
            functions: None,
            theme: None,
        };
        let schema = json!({"type": "object"});

        let ctx = build_prompt_context_v09(&catalog, &schema);
        assert_eq!(ctx.spec_version, SpecVersion::V0_9);
        assert!(ctx.protocol_description.contains("v0.9"));
        assert!(ctx
            .protocol_description
            .contains("https://example.com/basic.json"));
        assert!(ctx.catalog_schema_json.contains("object"));
        assert!(ctx.examples.contains("createSurface"));
    }

    #[test]
    fn test_build_prompt_context_v08() {
        let schema = json!({"type": "object"});

        let ctx = build_prompt_context_v08("https://example.com/standard.json", &schema);
        assert_eq!(ctx.spec_version, SpecVersion::V0_8);
        assert!(ctx.protocol_description.contains("v0.8"));
        assert!(ctx.examples.contains("beginRendering"));
    }

    #[test]
    fn test_prompt_context_display() {
        let catalog = Catalog {
            catalog_id: "test".to_string(),
            components: None,
            functions: None,
            theme: None,
        };
        let schema = json!({"type": "object"});
        let ctx = build_prompt_context_v09(&catalog, &schema);
        let display = format!("{}", ctx);
        assert!(display.contains("Catalog Schema"));
        assert!(display.contains("Examples"));
    }
}
