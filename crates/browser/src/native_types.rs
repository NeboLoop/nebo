use serde::{Deserialize, Serialize};

/// Message exchanged between the native messaging host and the Chrome extension.
/// Bidirectional — both sides send and receive these.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeMessage {
    #[serde(rename = "type")]
    pub msg_type: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,

    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension_id: Option<String>,
}

impl NativeMessage {
    pub fn ok(msg_type: &str) -> Self {
        Self {
            msg_type: msg_type.to_string(),
            id: None,
            tool: String::new(),
            args: None,
            result: None,
            error: None,
            version: None,
            extension_id: None,
        }
    }

    pub fn pong() -> Self {
        Self::ok("pong")
    }

    pub fn connected() -> Self {
        Self::ok("connected")
    }

    pub fn error_msg(err: &str) -> Self {
        Self {
            msg_type: "error".to_string(),
            id: None,
            tool: String::new(),
            args: None,
            result: None,
            error: Some(err.to_string()),
            version: None,
            extension_id: None,
        }
    }

    pub fn tool_request(id: i64, tool: &str, args: &serde_json::Value) -> Self {
        Self {
            msg_type: "execute_tool".to_string(),
            id: Some(id),
            tool: tool.to_string(),
            args: Some(args.clone()),
            result: None,
            error: None,
            version: None,
            extension_id: None,
        }
    }

    pub fn tool_response(id: i64, result: Result<serde_json::Value, String>) -> Self {
        match result {
            Ok(val) => Self {
                msg_type: "tool_response".to_string(),
                id: Some(id),
                tool: String::new(),
                args: None,
                result: Some(val),
                error: None,
                version: None,
                extension_id: None,
            },
            Err(err) => Self {
                msg_type: "tool_response".to_string(),
                id: Some(id),
                tool: String::new(),
                args: None,
                result: None,
                error: Some(err),
                version: None,
                extension_id: None,
            },
        }
    }

    pub fn show_indicators() -> Self {
        Self::ok("show_indicators")
    }

    pub fn hide_indicators() -> Self {
        Self::ok("hide_indicators")
    }
}
