use a2ui_types::common::SpecVersion;

use crate::error::A2uiError;

/// Validate an outgoing server-to-client message against a catalog schema.
///
/// This performs pre-send validation as recommended by the A2UI spec
/// (agent-side, before transmitting to the client).
///
/// # Arguments
/// * `message` — The serialized message as a `serde_json::Value`.
/// * `catalog_schema` — The JSON Schema of the catalog the surface uses.
/// * `surface_id` — The surface ID for error reporting.
///
/// # Returns
/// `Ok(())` if the message is valid, or `Err(A2uiError::Validation(...))` with structured errors.
pub fn validate_message(
    message: &serde_json::Value,
    catalog_schema: &serde_json::Value,
    surface_id: &str,
) -> Result<(), A2uiError> {
    a2ui_validation::validate(catalog_schema, message, surface_id).map_err(A2uiError::Validation)
}

/// Parse and validate an incoming client-to-server message (v0.9).
///
/// Deserializes raw JSON into the typed `ClientToServerMessage` and optionally
/// validates it.
///
/// # Arguments
/// * `raw_json` — The raw JSON bytes from the client.
///
/// # Returns
/// The parsed message, or a serialization error.
pub fn parse_client_message_v09(
    raw_json: &[u8],
) -> Result<a2ui_types::v09::client_to_server::ClientToServerMessage, A2uiError> {
    serde_json::from_slice(raw_json).map_err(A2uiError::Serialization)
}

/// Parse and validate an incoming client-to-server message (v0.8).
///
/// # Arguments
/// * `raw_json` — The raw JSON bytes from the client.
///
/// # Returns
/// The parsed message, or a serialization error.
pub fn parse_client_message_v08(
    raw_json: &[u8],
) -> Result<a2ui_types::v08::client_to_server::ClientToServerMessage, A2uiError> {
    serde_json::from_slice(raw_json).map_err(A2uiError::Serialization)
}

/// Parse an incoming client-to-server message, auto-detecting the version
/// from the JSON content.
///
/// v0.9 messages have a `"version": "v0.9"` field. v0.8 messages do not have a version field.
///
/// # Returns
/// An enum indicating which version was parsed.
pub fn parse_client_message_auto(raw_json: &[u8]) -> Result<ParsedClientMessage, A2uiError> {
    let value: serde_json::Value =
        serde_json::from_slice(raw_json).map_err(A2uiError::Serialization)?;

    if let Some(version) = value.get("version").and_then(|v| v.as_str()) {
        if version == "v0.9" {
            let msg: a2ui_types::v09::client_to_server::ClientToServerMessage =
                serde_json::from_value(value).map_err(A2uiError::Serialization)?;
            return Ok(ParsedClientMessage::V09(msg));
        }
    }

    // No version field or unknown version — try v0.8
    let msg: a2ui_types::v08::client_to_server::ClientToServerMessage =
        serde_json::from_value(value).map_err(A2uiError::Serialization)?;
    Ok(ParsedClientMessage::V08(msg))
}

/// A parsed client-to-server message tagged with its version.
#[derive(Debug, Clone, PartialEq)]
pub enum ParsedClientMessage {
    V08(a2ui_types::v08::client_to_server::ClientToServerMessage),
    V09(a2ui_types::v09::client_to_server::ClientToServerMessage),
}

impl ParsedClientMessage {
    /// Returns the spec version of the parsed message.
    pub fn spec_version(&self) -> SpecVersion {
        match self {
            ParsedClientMessage::V08(_) => SpecVersion::V0_8,
            ParsedClientMessage::V09(_) => SpecVersion::V0_9,
        }
    }
}

/// Serialize a v0.9 server-to-client message to JSON bytes.
pub fn serialize_message_v09(
    msg: &a2ui_types::v09::server_to_client::ServerToClientMessage,
) -> Result<Vec<u8>, A2uiError> {
    serde_json::to_vec(msg).map_err(A2uiError::Serialization)
}

/// Serialize a v0.8 server-to-client message to JSON bytes.
pub fn serialize_message_v08(
    msg: &a2ui_types::v08::server_to_client::ServerToClientMessage,
) -> Result<Vec<u8>, A2uiError> {
    serde_json::to_vec(msg).map_err(A2uiError::Serialization)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_v09_action() {
        let raw = serde_json::to_vec(&json!({
            "version": "v0.9",
            "action": {
                "name": "submit",
                "surfaceId": "s1",
                "sourceComponentId": "btn1",
                "timestamp": "2025-01-01T00:00:00Z",
                "context": {}
            }
        }))
        .unwrap();

        let msg = parse_client_message_v09(&raw).unwrap();
        assert!(msg.action.is_some());
        assert_eq!(msg.action.unwrap().name, "submit");
    }

    #[test]
    fn test_parse_v08_user_action() {
        let raw = serde_json::to_vec(&json!({
            "userAction": {
                "name": "submit",
                "surfaceId": "s1",
                "sourceComponentId": "btn1",
                "timestamp": "2025-01-01T00:00:00Z",
                "context": {}
            }
        }))
        .unwrap();

        let msg = parse_client_message_v08(&raw).unwrap();
        assert!(msg.user_action.is_some());
        assert_eq!(msg.user_action.unwrap().name, "submit");
    }

    #[test]
    fn test_parse_auto_detect_v09() {
        let raw = serde_json::to_vec(&json!({
            "version": "v0.9",
            "action": {
                "name": "click",
                "surfaceId": "s1",
                "sourceComponentId": "c1",
                "timestamp": "2025-01-01T00:00:00Z",
                "context": {}
            }
        }))
        .unwrap();

        let result = parse_client_message_auto(&raw).unwrap();
        assert_eq!(result.spec_version(), SpecVersion::V0_9);
    }

    #[test]
    fn test_parse_auto_detect_v08() {
        let raw = serde_json::to_vec(&json!({
            "userAction": {
                "name": "click",
                "surfaceId": "s1",
                "sourceComponentId": "c1",
                "timestamp": "2025-01-01T00:00:00Z",
                "context": {}
            }
        }))
        .unwrap();

        let result = parse_client_message_auto(&raw).unwrap();
        assert_eq!(result.spec_version(), SpecVersion::V0_8);
    }

    #[test]
    fn test_serialize_v09() {
        let msg = a2ui_types::v09::server_to_client::ServerToClientMessage {
            version: "v0.9".to_string(),
            create_surface: Some(a2ui_types::v09::server_to_client::CreateSurface {
                surface_id: "s1".into(),
                catalog_id: "https://example.com/basic.json".to_string(),
                theme: None,
                send_data_model: None,
            }),
            update_components: None,
            update_data_model: None,
            delete_surface: None,
        };

        let bytes = serialize_message_v09(&msg).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["version"], "v0.9");
        assert!(value.get("createSurface").is_some());
    }

    #[test]
    fn test_validate_message_valid() {
        let schema = json!({
            "type": "object",
            "properties": {
                "version": { "const": "v0.9" },
                "createSurface": { "type": "object" }
            },
            "required": ["version"]
        });
        let message = json!({
            "version": "v0.9",
            "createSurface": { "surfaceId": "s1", "catalogId": "c1" }
        });

        assert!(validate_message(&message, &schema, "s1").is_ok());
    }

    #[test]
    fn test_validate_message_invalid() {
        let schema = json!({
            "type": "object",
            "properties": {
                "version": { "const": "v0.9" }
            },
            "required": ["version"]
        });
        let message = json!({ "noversion": true });

        let result = validate_message(&message, &schema, "s1");
        assert!(result.is_err());
    }
}
