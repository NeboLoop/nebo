use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Error type for comm operations.
#[derive(Debug, thiserror::Error)]
pub enum CommError {
    #[error("not connected")]
    NotConnected,
    #[error("no active plugin")]
    NoActivePlugin,
    #[error("plugin not found: {0}")]
    PluginNotFound(String),
    #[error("{0}")]
    Other(String),
}

/// Thread-safe message handler callback.
pub type MessageHandler = Arc<dyn Fn(CommMessage) + Send + Sync>;

/// Type of a comm message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommMessageType {
    Message,
    Mention,
    Proposal,
    Command,
    Info,
    Task,
    TaskResult,
    TaskStatus,
    LoopChannel,
}

/// Lifecycle state of an A2A task (per NeboLoop A2A spec).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskStatus {
    Submitted,
    Working,
    Completed,
    Failed,
    Canceled,
    InputRequired,
}

/// One part of a task artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactPart {
    #[serde(rename = "type")]
    pub part_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Vec<u8>>,
}

/// Structured result from a completed A2A task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskArtifact {
    pub parts: Vec<ArtifactPart>,
}

/// A message in the comm layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommMessage {
    pub id: String,
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub topic: String,
    #[serde(default)]
    pub conversation_id: String,
    #[serde(rename = "type")]
    pub msg_type: CommMessageType,
    pub content: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
    #[serde(default)]
    pub timestamp: i64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub human_injected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_id: Option<String>,

    // A2A task lifecycle fields
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_status: Option<TaskStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<TaskArtifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Skill for A2A Agent Card discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCardSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// Provider info for agent card.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCardProvider {
    pub organization: String,
}

/// Agent Card for A2A discovery (follows A2A spec).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCard {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_transport: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_input_modes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_output_modes: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub capabilities: HashMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<AgentCardSkill>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<AgentCardProvider>,
}

/// Loop channel info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopChannelInfo {
    pub channel_id: String,
    pub channel_name: String,
    pub loop_id: String,
    pub loop_name: String,
}

/// Loop info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopInfo {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Channel message item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelMessageItem {
    pub id: String,
    pub from: String,
    pub content: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

/// Channel member item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelMemberItem {
    pub bot_id: String,
    pub bot_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default)]
    pub is_online: bool,
}

/// Manager status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagerStatus {
    pub plugin_name: String,
    pub connected: bool,
    pub topics: Vec<String>,
    pub agent_id: String,
}
