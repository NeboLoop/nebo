use serde_json::json;

// ---------------------------------------------------------------------------
// v0.8 round-trip tests
// ---------------------------------------------------------------------------

mod v08 {
    use super::*;
    use a2ui_types::v08::capabilities::*;
    use a2ui_types::v08::client_to_server::*;
    use a2ui_types::v08::data_model::*;
    use a2ui_types::v08::server_to_client::*;

    #[test]
    fn roundtrip_surface_update() {
        let raw = json!({
            "surfaceUpdate": {
                "surfaceId": "main",
                "components": [
                    {
                        "id": "root",
                        "component": {
                            "Column": {
                                "children": { "explicitList": ["greeting"] }
                            }
                        }
                    },
                    {
                        "id": "greeting",
                        "component": {
                            "Text": {
                                "text": { "literalString": "Hello, world!" }
                            }
                        }
                    }
                ]
            }
        });

        let msg: ServerToClientMessage = serde_json::from_value(raw.clone()).unwrap();
        assert!(msg.surface_update.is_some());
        let su = msg.surface_update.as_ref().unwrap();
        assert_eq!(su.surface_id.as_ref().unwrap().as_str(), "main");
        assert_eq!(su.components.len(), 2);

        let reserialized = serde_json::to_value(&msg).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_begin_rendering() {
        let raw = json!({
            "beginRendering": {
                "surfaceId": "main",
                "root": "root",
                "catalogId": "https://a2ui.org/specification/v0_8/standard_catalog_definition.json"
            }
        });

        let msg: ServerToClientMessage = serde_json::from_value(raw.clone()).unwrap();
        assert!(msg.begin_rendering.is_some());
        let br = msg.begin_rendering.as_ref().unwrap();
        assert_eq!(br.root, "root");
        assert_eq!(
            br.catalog_id.as_ref().unwrap(),
            "https://a2ui.org/specification/v0_8/standard_catalog_definition.json"
        );

        let reserialized = serde_json::to_value(&msg).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_data_model_update() {
        let raw = json!({
            "dataModelUpdate": {
                "surfaceId": "main",
                "path": "user",
                "contents": [
                    { "key": "name", "valueString": "Bob" },
                    { "key": "isVerified", "valueBoolean": true },
                    {
                        "key": "address",
                        "valueMap": [
                            { "key": "street", "valueString": "123 Main St" },
                            { "key": "city", "valueString": "Anytown" }
                        ]
                    }
                ]
            }
        });

        let msg: ServerToClientMessage = serde_json::from_value(raw.clone()).unwrap();
        assert!(msg.data_model_update.is_some());
        let dmu = msg.data_model_update.as_ref().unwrap();
        assert_eq!(dmu.surface_id.as_str(), "main");
        assert_eq!(dmu.contents.len(), 3);
        assert_eq!(dmu.contents[0].value_string.as_ref().unwrap(), "Bob");
        assert_eq!(dmu.contents[1].value_boolean.unwrap(), true);
        assert!(dmu.contents[2].value_map.is_some());

        let reserialized = serde_json::to_value(&msg).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_delete_surface() {
        let raw = json!({
            "deleteSurface": {
                "surfaceId": "main"
            }
        });

        let msg: ServerToClientMessage = serde_json::from_value(raw.clone()).unwrap();
        assert!(msg.delete_surface.is_some());
        assert_eq!(
            msg.delete_surface.as_ref().unwrap().surface_id.as_str(),
            "main"
        );

        let reserialized = serde_json::to_value(&msg).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_user_action() {
        let raw = json!({
            "userAction": {
                "name": "submit_form",
                "surfaceId": "main_content_area",
                "sourceComponentId": "submit_btn",
                "timestamp": "2025-09-19T17:05:00Z",
                "context": {
                    "userInput": "User input text",
                    "formId": "f-123"
                }
            }
        });

        let msg: ClientToServerMessage = serde_json::from_value(raw.clone()).unwrap();
        assert!(msg.user_action.is_some());
        let ua = msg.user_action.as_ref().unwrap();
        assert_eq!(ua.name, "submit_form");
        assert_eq!(ua.surface_id.as_str(), "main_content_area");

        let reserialized = serde_json::to_value(&msg).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_client_capabilities() {
        let raw = json!({
            "supportedCatalogIds": [
                "https://a2ui.org/specification/v0_8/standard_catalog_definition.json",
                "https://my-company.com/a2ui/v0.8/my_custom_catalog.json"
            ]
        });

        let caps: ClientCapabilities = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(caps.supported_catalog_ids.len(), 2);
        assert!(caps.inline_catalogs.is_none());

        let reserialized = serde_json::to_value(&caps).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_bound_value_literal() {
        let raw = json!({ "literalString": "Hello" });
        let bv: BoundValue = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(bv.literal_string.as_ref().unwrap(), "Hello");
        assert!(bv.path.is_none());
        let reserialized = serde_json::to_value(&bv).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_bound_value_path() {
        let raw = json!({ "path": "/user/name" });
        let bv: BoundValue = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(bv.path.as_ref().unwrap(), "/user/name");
        assert!(bv.literal_string.is_none());
        let reserialized = serde_json::to_value(&bv).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_bound_value_path_and_literal() {
        let raw = json!({ "path": "/user/name", "literalString": "Guest" });
        let bv: BoundValue = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(bv.path.as_ref().unwrap(), "/user/name");
        assert_eq!(bv.literal_string.as_ref().unwrap(), "Guest");
        let reserialized = serde_json::to_value(&bv).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_children_explicit_list() {
        let raw = json!({ "explicitList": ["a", "b", "c"] });
        let ch: Children = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(ch.explicit_list.as_ref().unwrap(), &["a", "b", "c"]);
        let reserialized = serde_json::to_value(&ch).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_children_template() {
        let raw = json!({
            "template": {
                "dataBinding": "/user/posts",
                "componentId": "post_template"
            }
        });
        let ch: Children = serde_json::from_value(raw.clone()).unwrap();
        let tpl = ch.template.as_ref().unwrap();
        assert_eq!(tpl.data_binding, "/user/posts");
        assert_eq!(tpl.component_id, "post_template");
        let reserialized = serde_json::to_value(&ch).unwrap();
        assert_eq!(reserialized, raw);
    }
}

// ---------------------------------------------------------------------------
// v0.9 round-trip tests
// ---------------------------------------------------------------------------

mod v09 {
    use super::*;
    use a2ui_types::v09::capabilities::*;
    use a2ui_types::v09::catalog::*;
    use a2ui_types::v09::client_data_model::*;
    use a2ui_types::v09::client_to_server::*;
    use a2ui_types::v09::common_types::*;
    use a2ui_types::v09::server_to_client::*;

    #[test]
    fn roundtrip_create_surface() {
        let raw = json!({
            "version": "v0.9",
            "createSurface": {
                "surfaceId": "user_profile_card",
                "catalogId": "https://a2ui.org/specification/v0_9/basic_catalog.json",
                "theme": { "primaryColor": "#00BFFF" },
                "sendDataModel": true
            }
        });

        let msg: ServerToClientMessage = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(msg.version, "v0.9");
        let cs = msg.create_surface.as_ref().unwrap();
        assert_eq!(cs.surface_id.as_str(), "user_profile_card");
        assert_eq!(
            cs.catalog_id,
            "https://a2ui.org/specification/v0_9/basic_catalog.json"
        );
        assert_eq!(cs.send_data_model, Some(true));

        let reserialized = serde_json::to_value(&msg).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_update_components() {
        let raw = json!({
            "version": "v0.9",
            "updateComponents": {
                "surfaceId": "user_profile_card",
                "components": [
                    {
                        "id": "root",
                        "component": "Column",
                        "children": ["user_name", "user_title"]
                    },
                    {
                        "id": "user_name",
                        "component": "Text",
                        "text": "John Doe"
                    }
                ]
            }
        });

        let msg: ServerToClientMessage = serde_json::from_value(raw.clone()).unwrap();
        let uc = msg.update_components.as_ref().unwrap();
        assert_eq!(uc.surface_id.as_str(), "user_profile_card");
        assert_eq!(uc.components.len(), 2);

        let reserialized = serde_json::to_value(&msg).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_update_data_model() {
        let raw = json!({
            "version": "v0.9",
            "updateDataModel": {
                "surfaceId": "user_profile_card",
                "path": "/user/name",
                "value": "Jane Doe"
            }
        });

        let msg: ServerToClientMessage = serde_json::from_value(raw.clone()).unwrap();
        let udm = msg.update_data_model.as_ref().unwrap();
        assert_eq!(udm.surface_id.as_str(), "user_profile_card");
        assert_eq!(udm.path.as_ref().unwrap(), "/user/name");
        assert_eq!(udm.value.as_ref().unwrap(), "Jane Doe");

        let reserialized = serde_json::to_value(&msg).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_delete_surface() {
        let raw = json!({
            "version": "v0.9",
            "deleteSurface": {
                "surfaceId": "user_profile_card"
            }
        });

        let msg: ServerToClientMessage = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(
            msg.delete_surface.as_ref().unwrap().surface_id.as_str(),
            "user_profile_card"
        );

        let reserialized = serde_json::to_value(&msg).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_action_message() {
        let raw = json!({
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
        });

        let msg: ClientToServerMessage = serde_json::from_value(raw.clone()).unwrap();
        let action = msg.action.as_ref().unwrap();
        assert_eq!(action.name, "submit_reservation");
        assert_eq!(action.surface_id.as_str(), "booking-surface");

        let reserialized = serde_json::to_value(&msg).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_error_message() {
        let raw = json!({
            "version": "v0.9",
            "error": {
                "code": "VALIDATION_FAILED",
                "surfaceId": "flight-status-card-123",
                "path": "/components/FlightCard/flightNumber",
                "message": "Missing required property 'flightNumber' in component 'FlightCard'."
            }
        });

        let msg: ClientToServerMessage = serde_json::from_value(raw.clone()).unwrap();
        let err = msg.error.as_ref().unwrap();
        assert_eq!(err.code, "VALIDATION_FAILED");
        assert_eq!(err.surface_id.as_str(), "flight-status-card-123");

        let reserialized = serde_json::to_value(&msg).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_client_capabilities() {
        let raw = json!({
            "v0.9": {
                "supportedCatalogIds": [
                    "https://a2ui.org/specification/v0_9/basic_catalog.json",
                    "https://my-company.com/catalogs/v1/custom.json"
                ]
            }
        });

        let caps: ClientCapabilities = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(caps.v09.supported_catalog_ids.len(), 2);

        let reserialized = serde_json::to_value(&caps).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_server_capabilities() {
        let raw = json!({
            "v0.9": {
                "supportedCatalogIds": [
                    "https://a2ui.org/specification/v0_9/basic_catalog.json"
                ],
                "acceptsInlineCatalogs": true
            }
        });

        let caps: ServerCapabilities = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(caps.v09.accepts_inline_catalogs, Some(true));

        let reserialized = serde_json::to_value(&caps).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_client_data_model() {
        let raw = json!({
            "version": "v0.9",
            "surfaces": {
                "booking-surface": {
                    "reservationTime": "7:00 PM",
                    "partySize": 4
                }
            }
        });

        let cdm: ClientDataModel = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(cdm.version, "v0.9");
        assert!(cdm.surfaces.contains_key("booking-surface"));

        let reserialized = serde_json::to_value(&cdm).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_catalog() {
        let raw = json!({
            "catalogId": "https://example.com/catalog.json",
            "components": {
                "Text": { "type": "object", "properties": { "text": { "type": "string" } } }
            },
            "functions": [
                {
                    "name": "required",
                    "parameters": { "type": "object" },
                    "returnType": "boolean"
                }
            ]
        });

        let catalog: Catalog = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(catalog.catalog_id, "https://example.com/catalog.json");
        assert!(catalog.components.is_some());
        assert_eq!(catalog.functions.as_ref().unwrap().len(), 1);
        assert_eq!(catalog.functions.as_ref().unwrap()[0].name, "required");

        let reserialized = serde_json::to_value(&catalog).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_dynamic_string_literal() {
        let raw = json!("Hello, world!");
        let ds: DynamicString = serde_json::from_value(raw.clone()).unwrap();
        assert!(matches!(ds, DynamicString::Literal(ref s) if s == "Hello, world!"));
        let reserialized = serde_json::to_value(&ds).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_dynamic_string_binding() {
        let raw = json!({ "path": "/user/name" });
        let ds: DynamicString = serde_json::from_value(raw.clone()).unwrap();
        assert!(matches!(ds, DynamicString::Binding(ref b) if b.path == "/user/name"));
        let reserialized = serde_json::to_value(&ds).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_child_list_static() {
        let raw = json!(["child1", "child2", "child3"]);
        let cl: ChildList = serde_json::from_value(raw.clone()).unwrap();
        assert!(matches!(cl, ChildList::Static(ref v) if v.len() == 3));
        let reserialized = serde_json::to_value(&cl).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_child_list_template() {
        let raw = json!({
            "componentId": "employee_card_template",
            "path": "/employees"
        });
        let cl: ChildList = serde_json::from_value(raw.clone()).unwrap();
        assert!(matches!(cl, ChildList::Template(ref t) if t.path == "/employees"));
        let reserialized = serde_json::to_value(&cl).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_action_server_event() {
        let raw = json!({
            "event": {
                "name": "submit_form",
                "context": { "itemId": "123" }
            }
        });
        let action: Action = serde_json::from_value(raw.clone()).unwrap();
        assert!(action.event.is_some());
        assert_eq!(action.event.as_ref().unwrap().name, "submit_form");
        let reserialized = serde_json::to_value(&action).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_action_local_function() {
        let raw = json!({
            "functionCall": {
                "call": "openUrl",
                "args": { "url": "https://example.com" }
            }
        });
        let action: Action = serde_json::from_value(raw.clone()).unwrap();
        assert!(action.function_call.is_some());
        assert_eq!(action.function_call.as_ref().unwrap().call, "openUrl");
        let reserialized = serde_json::to_value(&action).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_check_rule() {
        let raw = json!({
            "condition": { "path": "/formData/email" },
            "message": "Email is required."
        });
        let rule: CheckRule = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(rule.message, "Email is required.");
        let reserialized = serde_json::to_value(&rule).unwrap();
        assert_eq!(reserialized, raw);
    }

    #[test]
    fn roundtrip_function_call() {
        let raw = json!({
            "call": "formatString",
            "args": { "value": "Hello, ${/user/firstName}!" },
            "returnType": "string"
        });
        let fc: FunctionCall = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(fc.call, "formatString");
        assert_eq!(
            fc.return_type.as_ref().unwrap(),
            &FunctionReturnType::String
        );
        let reserialized = serde_json::to_value(&fc).unwrap();
        assert_eq!(reserialized, raw);
    }
}
