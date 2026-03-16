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
    /// Rotated bot JWT — use this token for the next reconnect.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub token: String,
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
    /// Agent ID for agent space / @mention deliveries.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent_id: String,
    /// Agent slug for agent space / @mention deliveries.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent_slug: String,
    /// Source channel ID for @mention deliveries routed from a channel.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_channel_id: String,
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
    /// Agent ID for agent space joins.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent_id: String,
    /// Agent slug for agent space joins.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent_slug: String,
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
            agent_id: String::new(),
            agent_slug: String::new(),
            source_channel_id: String::new(),
        };
        let json = serde_json::to_string(&p).unwrap();
        let p2: DeliveryPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(p2.sender_id, "sender-1");
        assert_eq!(p2.content["text"], "hello");
        // Agent fields should be omitted when empty
        assert!(!json.contains("agentId"));
    }

    #[test]
    fn test_delivery_with_agent_fields() {
        let p = DeliveryPayload {
            sender_id: "sender-1".into(),
            stream: "agent_space".into(),
            content: serde_json::json!({"text": "hello agent"}),
            agent_id: "agent-123".into(),
            agent_slug: "atlas".into(),
            source_channel_id: String::new(),
        };
        let json = serde_json::to_string(&p).unwrap();
        let p2: DeliveryPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(p2.agent_id, "agent-123");
        assert_eq!(p2.agent_slug, "atlas");
        assert!(p2.source_channel_id.is_empty());
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
            agent_id: String::new(),
            agent_slug: String::new(),
        };
        let json = serde_json::to_string(&p).unwrap();
        // Empty fields should be omitted
        assert!(!json.contains("peerId"));
        assert!(!json.contains("agentId"));
        let p2: JoinResultPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(p2.channel_name, "general");
    }

    #[test]
    fn test_join_result_with_agent_fields() {
        let p = JoinResultPayload {
            conversation_id: "conv-agent-1".into(),
            bot_id: String::new(),
            stream: String::new(),
            channel_id: String::new(),
            channel_name: String::new(),
            loop_id: "loop-1".into(),
            peer_id: String::new(),
            peer_type: String::new(),
            agent_id: "agent-456".into(),
            agent_slug: "researcher".into(),
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("agentId"));
        assert!(json.contains("researcher"));
        // Backward compat: deserialize without agent fields
        let minimal = r#"{"conversationId":"conv-1","senderId":"","stream":""}"#;
        let p2: JoinResultPayload = serde_json::from_str(minimal).unwrap();
        assert!(p2.agent_id.is_empty());
        assert!(p2.agent_slug.is_empty());
    }
}
