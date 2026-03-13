use serde::{Deserialize, Serialize, Serializer};

/// Serialize an i64 (0/1) as a JSON boolean.
fn i64_as_bool<S: Serializer>(val: &i64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_bool(*val != 0)
}

/// Serialize an Option<i64> (0/1/null) as a JSON boolean (defaults to false).
fn opt_i64_as_bool<S: Serializer>(val: &Option<i64>, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_bool(val.unwrap_or(0) != 0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub email: String,
    pub password_hash: String,
    pub name: String,
    pub avatar_url: Option<String>,
    pub email_verified: i64,
    pub email_verify_token: Option<String>,
    pub email_verify_expires: Option<i64>,
    pub password_reset_token: Option<String>,
    pub password_reset_expires: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserPreference {
    pub user_id: String,
    #[serde(serialize_with = "i64_as_bool")]
    pub email_notifications: i64,
    #[serde(serialize_with = "i64_as_bool")]
    pub marketing_emails: i64,
    pub timezone: String,
    pub language: String,
    pub theme: String,
    pub updated_at: i64,
    #[serde(serialize_with = "i64_as_bool")]
    pub inapp_notifications: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserProfile {
    pub user_id: String,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub location: Option<String>,
    pub timezone: Option<String>,
    pub occupation: Option<String>,
    pub interests: Option<String>,
    pub communication_style: Option<String>,
    pub goals: Option<String>,
    pub context: Option<String>,
    pub onboarding_completed: Option<i64>,
    pub onboarding_step: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub tool_permissions: Option<String>,
    pub terms_accepted_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshToken {
    pub id: String,
    pub user_id: String,
    pub token_hash: String,
    pub expires_at: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub name: Option<String>,
    pub scope: Option<String>,
    pub scope_id: Option<String>,
    pub summary: Option<String>,
    pub token_count: Option<i64>,
    pub message_count: Option<i64>,
    pub last_compacted_at: Option<i64>,
    pub metadata: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub compaction_count: Option<i64>,
    pub memory_flush_at: Option<i64>,
    pub memory_flush_compaction_count: Option<i64>,
    pub send_policy: Option<String>,
    pub model_override: Option<String>,
    pub provider_override: Option<String>,
    pub auth_profile_override: Option<String>,
    pub auth_profile_override_source: Option<String>,
    pub verbose_level: Option<String>,
    pub custom_label: Option<String>,
    pub last_embedded_message_id: Option<i64>,
    pub active_task: Option<String>,
    pub last_summarized_count: Option<i64>,
    pub work_tasks: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthProfile {
    pub id: String,
    pub name: String,
    pub provider: String,
    #[serde(skip_serializing)]
    pub api_key: String,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub priority: Option<i64>,
    #[serde(serialize_with = "opt_i64_as_bool")]
    pub is_active: Option<i64>,
    pub cooldown_until: Option<i64>,
    pub last_used_at: Option<i64>,
    pub usage_count: Option<i64>,
    pub error_count: Option<i64>,
    pub metadata: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub auth_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chat {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub user_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: String,
    pub chat_id: String,
    pub role: String,
    pub content: String,
    pub metadata: Option<String>,
    pub created_at: i64,
    pub day_marker: Option<String>,
    pub tool_calls: Option<String>,
    pub tool_results: Option<String>,
    pub token_estimate: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProfile {
    pub id: i64,
    pub name: String,
    pub personality_preset: Option<String>,
    pub custom_personality: Option<String>,
    pub voice_style: Option<String>,
    pub response_length: Option<String>,
    pub emoji_usage: Option<String>,
    pub formality: Option<String>,
    pub proactivity: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub emoji: Option<String>,
    pub creature: Option<String>,
    pub vibe: Option<String>,
    pub avatar: Option<String>,
    pub agent_rules: Option<String>,
    pub tool_notes: Option<String>,
    pub role: Option<String>,
    pub quiet_hours_start: String,
    pub quiet_hours_end: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Advisor {
    pub id: i64,
    pub name: String,
    pub role: String,
    pub description: String,
    pub priority: i64,
    #[serde(serialize_with = "i64_as_bool")]
    pub enabled: i64,
    #[serde(serialize_with = "i64_as_bool")]
    pub memory_access: i64,
    pub persona: String,
    pub timeout_seconds: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronJob {
    pub id: i64,
    pub name: String,
    pub schedule: String,
    pub command: String,
    pub task_type: String,
    pub message: Option<String>,
    pub deliver: Option<String>,
    #[serde(serialize_with = "opt_i64_as_bool")]
    pub enabled: Option<i64>,
    pub last_run: Option<String>,
    pub run_count: Option<i64>,
    pub last_error: Option<String>,
    pub created_at: Option<String>,
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronHistory {
    pub id: i64,
    pub job_id: i64,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    #[serde(serialize_with = "opt_i64_as_bool")]
    pub success: Option<i64>,
    pub output: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Memory {
    pub id: i64,
    pub namespace: String,
    pub key: String,
    pub value: String,
    pub tags: Option<String>,
    pub metadata: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub accessed_at: Option<String>,
    pub access_count: Option<i64>,
    pub user_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryChunk {
    pub id: i64,
    pub memory_id: Option<i64>,
    pub chunk_index: i64,
    pub text: String,
    pub source: Option<String>,
    pub path: Option<String>,
    pub start_char: Option<i64>,
    pub end_char: Option<i64>,
    pub model: Option<String>,
    pub user_id: String,
    pub created_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEmbedding {
    pub id: i64,
    pub chunk_id: Option<i64>,
    pub model: String,
    pub dimensions: i64,
    pub embedding: Vec<u8>,
    pub created_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingCache {
    pub content_hash: String,
    pub embedding: Vec<u8>,
    pub model: String,
    pub dimensions: i64,
    pub created_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub id: String,
    pub user_id: String,
    #[serde(rename = "type")]
    pub notification_type: String,
    pub title: String,
    pub body: Option<String>,
    pub action_url: Option<String>,
    pub icon: Option<String>,
    pub read_at: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub channel_type: String,
    #[serde(serialize_with = "opt_i64_as_bool")]
    pub is_enabled: Option<i64>,
    pub connection_status: Option<String>,
    pub last_connected_at: Option<i64>,
    pub last_error: Option<String>,
    pub message_count: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpIntegration {
    pub id: String,
    pub name: String,
    pub server_type: String,
    pub server_url: Option<String>,
    pub auth_type: String,
    #[serde(serialize_with = "opt_i64_as_bool")]
    pub is_enabled: Option<i64>,
    pub connection_status: Option<String>,
    pub last_connected_at: Option<i64>,
    pub last_error: Option<String>,
    pub metadata: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub tool_count: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingTask {
    pub id: String,
    pub task_type: String,
    pub status: String,
    pub session_key: String,
    pub user_id: Option<String>,
    pub prompt: String,
    pub system_prompt: Option<String>,
    pub description: Option<String>,
    pub lane: Option<String>,
    pub priority: Option<i64>,
    pub attempts: Option<i64>,
    pub max_attempts: Option<i64>,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub parent_task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Setting {
    #[serde(skip_serializing)]
    pub id: i64,
    #[serde(serialize_with = "i64_as_bool")]
    pub autonomous_mode: i64,
    #[serde(serialize_with = "i64_as_bool")]
    pub auto_approve_read: i64,
    #[serde(serialize_with = "i64_as_bool")]
    pub auto_approve_write: i64,
    #[serde(serialize_with = "i64_as_bool")]
    pub auto_approve_bash: i64,
    pub heartbeat_interval_minutes: i64,
    #[serde(serialize_with = "i64_as_bool")]
    pub comm_enabled: i64,
    pub comm_plugin: String,
    #[serde(serialize_with = "i64_as_bool")]
    pub developer_mode: i64,
    #[serde(serialize_with = "i64_as_bool")]
    pub auto_update: i64,
    #[serde(skip_serializing)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginRegistry {
    pub id: String,
    pub name: String,
    pub plugin_type: String,
    pub display_name: String,
    pub description: String,
    pub icon: String,
    pub version: String,
    #[serde(serialize_with = "i64_as_bool")]
    pub is_enabled: i64,
    #[serde(serialize_with = "i64_as_bool")]
    pub is_installed: i64,
    pub settings_manifest: String,
    pub connection_status: String,
    pub last_connected_at: Option<i64>,
    pub last_error: Option<String>,
    pub metadata: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSetting {
    pub id: String,
    pub plugin_id: String,
    pub setting_key: String,
    pub setting_value: String,
    #[serde(serialize_with = "i64_as_bool")]
    pub is_secret: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModel {
    pub id: String,
    pub provider: String,
    pub model_id: String,
    pub display_name: String,
    #[serde(serialize_with = "opt_i64_as_bool")]
    pub is_active: Option<i64>,
    #[serde(serialize_with = "opt_i64_as_bool")]
    pub is_default: Option<i64>,
    pub context_window: Option<i64>,
    pub input_price: Option<f64>,
    pub output_price: Option<f64>,
    pub capabilities: Option<String>,
    pub kind: Option<String>,
    #[serde(serialize_with = "opt_i64_as_bool")]
    pub preferred: Option<i64>,
    pub seeded_version: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OauthConnection {
    pub id: String,
    pub user_id: String,
    pub provider: String,
    pub provider_user_id: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorLog {
    pub id: i64,
    pub timestamp: i64,
    pub level: String,
    pub module: String,
    pub message: String,
    pub stacktrace: Option<String>,
    pub context: Option<String>,
    pub resolved: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lead {
    pub id: String,
    pub email: String,
    pub name: Option<String>,
    pub source: Option<String>,
    pub status: String,
    pub metadata: Option<String>,
    pub created_at: i64,
}

// ── Workflows ──

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    pub id: String,
    pub code: Option<String>,
    pub name: String,
    pub version: String,
    pub definition: String,
    pub skill_md: Option<String>,
    pub manifest: Option<String>,
    pub is_enabled: i64,
    pub installed_at: i64,
    pub updated_at: i64,
    /// Path to .napp archive or user directory on filesystem.
    pub napp_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowToolBinding {
    pub id: i64,
    pub workflow_id: String,
    pub interface_name: String,
    #[serde(rename = "tool", alias = "toolCode")]
    pub tool_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRun {
    pub id: String,
    pub workflow_id: String,
    pub trigger_type: String,
    pub trigger_detail: Option<String>,
    pub status: String,
    pub inputs: Option<String>,
    pub current_activity: Option<String>,
    pub total_tokens_used: Option<i64>,
    pub error: Option<String>,
    pub error_activity: Option<String>,
    pub session_key: Option<String>,
    pub started_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowActivityResult {
    pub id: i64,
    pub run_id: String,
    pub activity_id: String,
    pub status: String,
    pub tokens_used: Option<i64>,
    pub attempts: Option<i64>,
    pub error: Option<String>,
    pub started_at: i64,
    pub completed_at: Option<i64>,
}

// ── Roles ──

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Role {
    pub id: String,
    pub code: Option<String>,
    pub name: String,
    pub description: String,
    pub role_md: String,
    pub frontmatter: String,
    pub pricing_model: Option<String>,
    pub pricing_cost: Option<f64>,
    pub is_enabled: i64,
    pub installed_at: i64,
    pub updated_at: i64,
    /// Path to .napp archive or user directory on filesystem.
    pub napp_path: Option<String>,
}

/// A workflow binding owned by a role, with trigger configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleWorkflow {
    pub id: i64,
    pub role_id: String,
    pub binding_name: String,
    pub workflow_ref: String,
    pub workflow_id: Option<String>,
    pub trigger_type: String,
    pub trigger_config: String,
    pub description: Option<String>,
    pub inputs: Option<String>,
    #[serde(serialize_with = "i64_as_bool")]
    pub is_active: i64,
}

// ── Entity Config ──

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityConfig {
    pub id: i64,
    pub entity_type: String,
    pub entity_id: String,
    pub heartbeat_enabled: Option<i64>,
    pub heartbeat_interval_minutes: Option<i64>,
    pub heartbeat_content: Option<String>,
    pub heartbeat_window_start: Option<String>,
    pub heartbeat_window_end: Option<String>,
    pub permissions: Option<String>,
    pub resource_grants: Option<String>,
    pub model_preference: Option<String>,
    pub personality_snippet: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}
