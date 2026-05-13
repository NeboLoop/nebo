use std::sync::{Arc, Mutex};

use a2ui_core::error::A2uiError;
use a2ui_core::message::*;
use a2ui_core::negotiation::*;
use a2ui_core::prompt::*;
use a2ui_core::traits::*;
use a2ui_core::validation::*;
use a2ui_types::common::CatalogId;
use a2ui_types::v09::catalog::Catalog;

use serde_json::json;

// ---------------------------------------------------------------------------
// Mock implementations
// ---------------------------------------------------------------------------

struct MockCatalogProvider {
    catalogs: Vec<(CatalogId, Catalog, serde_json::Value)>,
}

impl CatalogProvider for MockCatalogProvider {
    fn available_catalogs(&self) -> Vec<CatalogInfo> {
        self.catalogs
            .iter()
            .map(|(id, _, _)| CatalogInfo {
                catalog_id: id.clone(),
                description: None,
            })
            .collect()
    }

    fn get_catalog(&self, id: &CatalogId) -> Option<Catalog> {
        self.catalogs
            .iter()
            .find(|(cid, _, _)| cid == id)
            .map(|(_, cat, _)| cat.clone())
    }

    fn get_catalog_schema(&self, id: &CatalogId) -> Option<serde_json::Value> {
        self.catalogs
            .iter()
            .find(|(cid, _, _)| cid == id)
            .map(|(_, _, schema)| schema.clone())
    }
}

struct MockTransport {
    sent: Arc<Mutex<Vec<Vec<u8>>>>,
}

#[async_trait::async_trait]
impl ClientTransport for MockTransport {
    async fn send_to_client(
        &self,
        msg: &[u8],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.sent.lock().unwrap().push(msg.to_vec());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Integration: Full v0.9 flow
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_v09_flow_negotiate_build_validate_send() {
    // 1. Set up catalog provider
    let catalog = Catalog {
        catalog_id: "https://a2ui.org/specification/v0_9/basic_catalog.json".to_string(),
        components: Some(json!({
            "Text": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "component": { "const": "Text" },
                    "text": {}
                },
                "required": ["id", "component"]
            },
            "Column": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "component": { "const": "Column" },
                    "children": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["id", "component"]
            }
        })),
        functions: None,
        theme: None,
    };

    let catalog_schema = json!({
        "type": "object",
        "properties": {
            "version": { "const": "v0.9" },
            "createSurface": { "type": "object" },
            "updateComponents": { "type": "object" },
            "updateDataModel": { "type": "object" },
            "deleteSurface": { "type": "object" }
        },
        "required": ["version"]
    });

    let provider = MockCatalogProvider {
        catalogs: vec![(
            CatalogId::new("https://a2ui.org/specification/v0_9/basic_catalog.json"),
            catalog.clone(),
            catalog_schema.clone(),
        )],
    };

    // 2. Negotiate catalog
    let client_supported =
        vec!["https://a2ui.org/specification/v0_9/basic_catalog.json".to_string()];

    let negotiation_result = negotiate_catalog(&client_supported, &provider).unwrap();
    assert_eq!(
        negotiation_result.catalog_id.as_str(),
        "https://a2ui.org/specification/v0_9/basic_catalog.json"
    );

    // 3. Build prompt context for LLM
    let prompt_ctx = build_prompt_context_v09(&catalog, &catalog_schema);
    assert!(prompt_ctx.protocol_description.contains("v0.9"));
    assert!(prompt_ctx.catalog_schema_json.contains("version"));

    // 4. Build messages
    let create_msg =
        CreateSurfaceBuilder::new("booking-surface", negotiation_result.catalog_id.as_str())
            .send_data_model(true)
            .build();

    let update_msg = UpdateComponentsBuilder::new("booking-surface")
        .add_component(json!({"id": "root", "component": "Column", "children": ["greeting"]}))
        .add_component(json!({"id": "greeting", "component": "Text", "text": "Hello!"}))
        .build();

    let data_msg = UpdateDataModelBuilder::new("booking-surface")
        .path("/user/name")
        .value(json!("Alice"))
        .build();

    // 5. Validate messages
    let create_json = serde_json::to_value(&create_msg).unwrap();
    assert!(validate_message(&create_json, &catalog_schema, "booking-surface").is_ok());

    let update_json = serde_json::to_value(&update_msg).unwrap();
    assert!(validate_message(&update_json, &catalog_schema, "booking-surface").is_ok());

    let data_json = serde_json::to_value(&data_msg).unwrap();
    assert!(validate_message(&data_json, &catalog_schema, "booking-surface").is_ok());

    // 6. Serialize and send via transport
    let sent_messages = Arc::new(Mutex::new(Vec::new()));
    let transport = MockTransport {
        sent: sent_messages.clone(),
    };

    let bytes = serialize_message_v09(&create_msg).unwrap();
    transport.send_to_client(&bytes).await.unwrap();

    let bytes = serialize_message_v09(&update_msg).unwrap();
    transport.send_to_client(&bytes).await.unwrap();

    let bytes = serialize_message_v09(&data_msg).unwrap();
    transport.send_to_client(&bytes).await.unwrap();

    let sent = sent_messages.lock().unwrap();
    assert_eq!(sent.len(), 3);

    // Verify what was sent is valid JSON
    for msg_bytes in sent.iter() {
        let _: serde_json::Value = serde_json::from_slice(msg_bytes).unwrap();
    }
}

// ---------------------------------------------------------------------------
// Integration: Full v0.8 flow
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_v08_flow() {
    let sent_messages = Arc::new(Mutex::new(Vec::new()));
    let transport = MockTransport {
        sent: sent_messages.clone(),
    };

    // 1. Build messages
    let begin_msg = BeginRenderingBuilder::new("main", "root")
        .catalog_id("https://a2ui.org/specification/v0_8/standard_catalog_definition.json")
        .build();

    let surface_msg = SurfaceUpdateBuilder::new()
        .surface_id("main")
        .add_component(a2ui_types::v08::server_to_client::ComponentInstance {
            id: "root".to_string(),
            weight: None,
            component: json!({"Column": {"children": {"explicitList": ["greeting"]}}}),
        })
        .add_component(a2ui_types::v08::server_to_client::ComponentInstance {
            id: "greeting".to_string(),
            weight: None,
            component: json!({"Text": {"text": {"literalString": "Hello, world!"}}}),
        })
        .build();

    let data_msg = DataModelUpdateBuilder::new("main")
        .add_entry(a2ui_types::v08::data_model::DataEntry {
            key: "username".to_string(),
            value_string: Some("Alice".to_string()),
            value_number: None,
            value_boolean: None,
            value_map: None,
        })
        .build();

    // 2. Serialize and send
    let bytes = serialize_message_v08(&begin_msg).unwrap();
    transport.send_to_client(&bytes).await.unwrap();

    let bytes = serialize_message_v08(&surface_msg).unwrap();
    transport.send_to_client(&bytes).await.unwrap();

    let bytes = serialize_message_v08(&data_msg).unwrap();
    transport.send_to_client(&bytes).await.unwrap();

    let sent = sent_messages.lock().unwrap();
    assert_eq!(sent.len(), 3);

    // Verify all sent messages are valid JSON
    for msg_bytes in sent.iter() {
        let _: serde_json::Value = serde_json::from_slice(msg_bytes).unwrap();
    }
}

// ---------------------------------------------------------------------------
// Integration: Parse client messages and handle errors
// ---------------------------------------------------------------------------

#[test]
fn parse_and_route_client_action_v09() {
    let raw = serde_json::to_vec(&json!({
        "version": "v0.9",
        "action": {
            "name": "submit_reservation",
            "surfaceId": "booking-surface",
            "sourceComponentId": "submit-btn",
            "timestamp": "2026-02-25T10:40:00Z",
            "context": {
                "time": "7:00 PM",
                "size": 4
            }
        }
    }))
    .unwrap();

    let parsed = parse_client_message_auto(&raw).unwrap();
    assert_eq!(parsed.spec_version(), a2ui_types::common::SpecVersion::V0_9);

    match parsed {
        ParsedClientMessage::V09(msg) => {
            let action = msg.action.unwrap();
            assert_eq!(action.name, "submit_reservation");
            assert_eq!(action.context["time"], "7:00 PM");
            assert_eq!(action.context["size"], 4);
        }
        _ => panic!("expected v0.9 message"),
    }
}

#[test]
fn parse_client_validation_error_v09() {
    let raw = serde_json::to_vec(&json!({
        "version": "v0.9",
        "error": {
            "code": "VALIDATION_FAILED",
            "surfaceId": "booking-surface",
            "path": "/components/0/children",
            "message": "Expected array of strings, got null."
        }
    }))
    .unwrap();

    let parsed = parse_client_message_v09(&raw).unwrap();
    let err = parsed.error.unwrap();
    assert_eq!(err.code, "VALIDATION_FAILED");
    assert_eq!(err.message, "Expected array of strings, got null.");
}

// ---------------------------------------------------------------------------
// Integration: Validation catches invalid messages
// ---------------------------------------------------------------------------

#[test]
fn validation_catches_bad_message() {
    let strict_schema = json!({
        "type": "object",
        "properties": {
            "version": { "const": "v0.9" },
            "createSurface": {
                "type": "object",
                "properties": {
                    "surfaceId": { "type": "string" },
                    "catalogId": { "type": "string" }
                },
                "required": ["surfaceId", "catalogId"]
            }
        },
        "required": ["version"]
    });

    // Missing version field
    let bad_msg = json!({"createSurface": {"surfaceId": "s1", "catalogId": "c1"}});
    let result = validate_message(&bad_msg, &strict_schema, "s1");
    assert!(result.is_err());

    match result.unwrap_err() {
        A2uiError::Validation(errors) => {
            assert!(!errors.is_empty());
            assert_eq!(errors[0].code, "VALIDATION_FAILED");
        }
        other => panic!("expected Validation error, got: {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Integration: Catalog negotiation with multiple catalogs
// ---------------------------------------------------------------------------

#[test]
fn negotiation_picks_highest_priority_client_catalog() {
    let provider = MockCatalogProvider {
        catalogs: vec![
            (
                CatalogId::new("https://a2ui.org/specification/v0_9/basic_catalog.json"),
                Catalog {
                    catalog_id: "https://a2ui.org/specification/v0_9/basic_catalog.json"
                        .to_string(),
                    components: None,
                    functions: None,
                    theme: None,
                },
                json!({}),
            ),
            (
                CatalogId::new("https://company.com/advanced.json"),
                Catalog {
                    catalog_id: "https://company.com/advanced.json".to_string(),
                    components: None,
                    functions: None,
                    theme: None,
                },
                json!({}),
            ),
        ],
    };

    // Client prefers advanced, then basic
    let client_ids = vec![
        "https://company.com/advanced.json".to_string(),
        "https://a2ui.org/specification/v0_9/basic_catalog.json".to_string(),
    ];

    let result = negotiate_catalog(&client_ids, &provider).unwrap();
    assert_eq!(
        result.catalog_id.as_str(),
        "https://company.com/advanced.json"
    );
}
