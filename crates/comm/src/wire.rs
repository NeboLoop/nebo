//! JSON payload types for the NeboLoop comms binary protocol.
//! Both the gateway and Rust SDK use these — single source of truth.

use serde::{Deserialize, Serialize};

/// CONNECT frame payload (client -> server).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// AUTH_OK / AUTH_FAIL frame payload (server -> client).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthResultPayload {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bot_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub plan: String,
}

/// SEND_MESSAGE frame payload (client -> server).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendPayload {
    pub conversation_id: String,
    pub stream: String,
    pub content: serde_json::Value,
}

/// MESSAGE_DELIVERY frame payload (server -> client).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryPayload {
    pub sender_id: String,
    pub stream: String,
    pub content: serde_json::Value,
}

/// JOIN_CONVERSATION frame payload (client -> server).
/// Either conversation_id OR (bot_id + stream) OR channel_id must be set.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinPayload {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub conversation_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bot_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stream: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub channel_id: String,
    #[serde(default)]
    pub last_acked_seq: u64,
}

/// JOIN result payload (server -> client).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinResultPayload {
    pub conversation_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bot_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stream: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub channel_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub channel_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub loop_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub peer_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub peer_type: String, // "bot" or "person"
}

/// LEAVE_CONVERSATION frame payload (client -> server).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeavePayload {
    pub conversation_id: String,
}

/// ACK frame payload (client -> server).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AckPayload {
    pub conversation_id: String,
    pub acked_seq: u64,
}

/// REPLAY frame payload (server -> client).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayPayload {
    pub conversation_id: String,
    pub from_seq: u64,
    pub to_seq: u64,
    pub message_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connect_roundtrip() {
        let p = ConnectPayload {
            bot_id: Some("bot-123".into()),
            token: Some("jwt-token".into()),
        };
        let json = serde_json::to_string(&p).unwrap();
        let p2: ConnectPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(p2.bot_id.as_deref(), Some("bot-123"));
    }

    #[test]
    fn test_delivery_roundtrip() {
        let p = DeliveryPayload {
            sender_id: "sender-1".into(),
            stream: "chat".into(),
            content: serde_json::json!({"text": "hello"}),
        };
        let json = serde_json::to_string(&p).unwrap();
        let p2: DeliveryPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(p2.sender_id, "sender-1");
        assert_eq!(p2.content["text"], "hello");
    }

    #[test]
    fn test_join_result_roundtrip() {
        let p = JoinResultPayload {
            conversation_id: "conv-1".into(),
            bot_id: String::new(),
            stream: String::new(),
            channel_id: "chan-1".into(),
            channel_name: "general".into(),
            loop_id: "loop-1".into(),
            peer_id: String::new(),
            peer_type: String::new(),
        };
        let json = serde_json::to_string(&p).unwrap();
        // Empty fields should be omitted
        assert!(!json.contains("peerId"));
        let p2: JoinResultPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(p2.channel_name, "general");
    }
}
