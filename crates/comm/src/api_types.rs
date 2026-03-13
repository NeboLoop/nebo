//! DTOs for the NeboLoop REST API covering all 5 hierarchy layers + loops.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── App / Tool Types ─────────────────────────────────────────────────

/// Developer who published an app or skill.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Author {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub verified: bool,
}

/// Compact app representation in list responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppItem {
    pub id: String,
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub version: String,
    pub author: Author,
    #[serde(default)]
    pub install_count: i64,
    #[serde(default)]
    pub rating: f64,
    #[serde(default)]
    pub review_count: i64,
    #[serde(default)]
    pub is_installed: bool,
    #[serde(default)]
    pub status: String,
}

/// Full app detail with manifest (GET /apps/{id}).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppDetail {
    #[serde(flatten)]
    pub item: AppItem,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub age_rating: Option<String>,
    #[serde(default)]
    pub platforms: Vec<String>,
    #[serde(default)]
    pub size: HashMap<String, i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default)]
    pub screenshots: Vec<String>,
    #[serde(default)]
    pub changelog: Vec<ChangelogEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub website_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub privacy_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub support_url: Option<String>,
}

/// Single changelog entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangelogEntry {
    pub version: String,
    pub date: String,
    pub notes: String,
}

/// Paginated list response for GET /api/v1/apps.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppsResponse {
    pub apps: Vec<AppItem>,
    #[serde(default)]
    pub total_count: i64,
    #[serde(default)]
    pub page: i64,
    #[serde(default)]
    pub page_size: i64,
}

// ── Review Types ─────────────────────────────────────────────────────

/// Single user review.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub id: String,
    pub user_name: String,
    pub rating: i64,
    pub title: String,
    pub body: String,
    pub created_at: String,
    #[serde(default)]
    pub helpful: i64,
}

/// Paginated response for GET /api/v1/apps/{id}/reviews.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewsResponse {
    pub reviews: Vec<Review>,
    #[serde(default)]
    pub total_count: i64,
    #[serde(default)]
    pub average: f64,
    #[serde(default)]
    pub distribution: [i64; 5],
}

// ── Skill Types ──────────────────────────────────────────────────────

/// Compact skill in list responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillItem {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: Author,
    #[serde(default)]
    pub install_count: i64,
    #[serde(default)]
    pub rating: f64,
    #[serde(default)]
    pub review_count: i64,
    #[serde(default)]
    pub is_installed: bool,
    #[serde(default)]
    pub status: String,
}

/// Full skill detail with manifest.
///
/// Also used for workflow and role detail — all artifact types share
/// the `GET /api/v1/skills/{id}` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDetail {
    #[serde(flatten)]
    pub item: SkillItem,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_url: Option<String>,
    /// Primary content: SKILL.md, WORKFLOW.md, or ROLE.md text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<serde_json::Value>,
    /// Secondary markdown content (marketplace description).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_md: Option<String>,
    /// Artifact type: "skill", "workflow", or "role".
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub artifact_type: Option<String>,
    /// Install code (e.g. SKIL-XXXX-XXXX).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Type-specific config (workflow definition JSON for workflows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_config: Option<serde_json::Value>,
    /// URL to download the sealed .napp archive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
}

/// Paginated list response for GET /api/v1/skills.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsResponse {
    pub skills: Vec<SkillItem>,
    #[serde(default)]
    pub total_count: i64,
    #[serde(default)]
    pub page: i64,
    #[serde(default)]
    pub page_size: i64,
}

// ── Workflow Types ────────────────────────────────────────────────────

/// Compact workflow in list responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowItem {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version: String,
    pub author: Author,
    #[serde(default)]
    pub install_count: i64,
    #[serde(default)]
    pub is_installed: bool,
}

/// Full workflow detail.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDetail {
    #[serde(flatten)]
    pub item: WorkflowItem,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_md: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<serde_json::Value>,
}

/// Paginated list response for workflows.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowsResponse {
    pub workflows: Vec<WorkflowItem>,
    #[serde(default)]
    pub total_count: i64,
    #[serde(default)]
    pub page: i64,
    #[serde(default)]
    pub page_size: i64,
}

// ── Role Types ───────────────────────────────────────────────────────

/// Compact role in list responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleItem {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub workflows: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing: Option<RolePricingInfo>,
}

/// Pricing info attached to a role.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RolePricingInfo {
    pub model: String,
    pub cost: f64,
}

/// Full role detail.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleDetail {
    #[serde(flatten)]
    pub item: RoleItem,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_md: Option<String>,
}

/// Paginated list response for roles.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RolesResponse {
    pub roles: Vec<RoleItem>,
    #[serde(default)]
    pub total_count: i64,
}

// ── Install Types ────────────────────────────────────────────────────

/// App data returned in an install response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallResponseApp {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<serde_json::Value>,
}

/// Returned by POST /api/v1/{apps,skills,workflows,roles}/{id}/install.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallResponse {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<InstallResponseApp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<InstallResponseApp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<InstallResponseApp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<InstallResponseApp>,
    #[serde(default)]
    pub installed_at: String,
    #[serde(default)]
    pub update_available: bool,
    #[serde(default)]
    pub platforms: Vec<String>,
    #[serde(default)]
    pub download_urls: HashMap<String, String>,
}

impl InstallResponse {
    /// Platform-specific download URL (e.g. "darwin-arm64").
    pub fn download_url(&self, api_server: &str, fallback_path: &str) -> String {
        let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
        if let Some(u) = self.download_urls.get(&platform) {
            format!("{}{}", api_server, u)
        } else {
            format!("{}{}", api_server, fallback_path)
        }
    }
}

// ── Code Redemption Types ────────────────────────────────────────────

/// Returned by POST /api/v1/codes/redeem — universal code redemption.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeRedeemResponse {
    /// "installed" or "payment_required"
    pub status: String,
    pub artifact: CodeRedeemArtifact,
    #[serde(default)]
    pub installs: Vec<CodeRedeemInstall>,
    /// Present when status == "payment_required"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<serde_json::Value>,
    /// URL to download the sealed .napp archive (present for approved artifacts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeRedeemArtifact {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub artifact_type: String,
    pub code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeRedeemInstall {
    pub bot_id: String,
    pub status: String,
}

// ── Connection Code Types ────────────────────────────────────────────

/// Sent to POST /api/v1/bots/connect/redeem.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedeemCodeRequest {
    pub code: String,
    pub name: String,
    pub purpose: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bot_id: String,
}

/// Returned by POST /api/v1/bots/connect/redeem.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedeemCodeResponse {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub purpose: String,
    pub visibility: String,
    pub connection_token: String,
    pub owner_email: String,
    pub owner_display_name: String,
}

// ── Bot Identity ─────────────────────────────────────────────────────

/// Sent to PUT /api/v1/bots/{id}.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateBotIdentityRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub role: String,
}

// ── Loop Types ───────────────────────────────────────────────────────

/// A loop the bot belongs to.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Loop {
    pub loop_id: String,
    pub loop_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub member_count: i64,
}

/// Loop membership with role info.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopMembership {
    pub loop_id: String,
    pub loop_name: String,
    #[serde(default)]
    pub loop_type: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub joined_at: String,
}

/// Detailed loop info.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopDetail {
    pub loop_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub is_public: bool,
    #[serde(default)]
    pub member_count: i64,
    #[serde(default)]
    pub my_role: String,
    #[serde(default)]
    pub joined_at: String,
}

/// Returned by GET /api/v1/bots/{id}/loops.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopsResponse {
    pub loops: Vec<Loop>,
}

/// Loop member with presence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopMember {
    pub bot_id: String,
    #[serde(default)]
    pub bot_name: String,
    #[serde(default)]
    pub bot_slug: String,
    #[serde(default)]
    pub purpose: String,
    #[serde(default)]
    pub reputation: i64,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub joined_at: String,
    #[serde(default)]
    pub is_online: bool,
}

/// Returned by GET /api/v1/bots/{id}/loops/{loopID}/members.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopMembersResponse {
    pub members: Vec<LoopMember>,
}

/// Sent to POST /api/v1/loops/join.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinLoopRequest {
    pub code: String,
    pub bot_id: String,
}

/// Returned by POST /api/v1/loops/join.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinLoopResponse {
    pub id: String,
    pub name: String,
}

// ── Channel Types ────────────────────────────────────────────────────

/// Channel the bot belongs to within a Loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopChannel {
    pub channel_id: String,
    pub channel_name: String,
    pub loop_id: String,
    pub loop_name: String,
    #[serde(default)]
    pub conversation_id: String,
}

/// Returned by GET /api/v1/bots/{id}/channels.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopChannelsResponse {
    pub channels: Vec<LoopChannel>,
}

/// Channel member with presence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelMember {
    pub bot_id: String,
    #[serde(default)]
    pub bot_name: String,
    #[serde(default)]
    pub bot_slug: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub is_online: bool,
}

/// Returned by GET /api/v1/bots/{id}/channels/{channelID}/members.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelMembersResponse {
    pub members: Vec<ChannelMember>,
}

/// Raw channel message from API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelMessageRaw {
    pub msg_id: String,
    pub sender_id: String,
    pub payload: String,
    pub created_at: String,
    #[serde(default)]
    pub seq: i64,
    #[serde(default)]
    pub stream: String,
}

/// Returned by GET /api/v1/bots/{id}/channels/{channelID}/messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelMessagesResponse {
    pub messages: Vec<ChannelMessageRaw>,
}

impl ChannelMessagesResponse {
    /// Normalize raw messages into clean items.
    pub fn normalize(&self) -> Vec<NormalizedChannelMessage> {
        self.messages
            .iter()
            .map(|raw| {
                let mut item = NormalizedChannelMessage {
                    id: raw.msg_id.clone(),
                    from: raw.sender_id.clone(),
                    content: String::new(),
                    created_at: raw.created_at.clone(),
                    role: None,
                };
                // Parse nested payload JSON
                if let Ok(p) = serde_json::from_str::<ChannelPayload>(&raw.payload) {
                    item.content = p.content.text;
                    if !p.metadata.role.is_empty() {
                        item.role = Some(p.metadata.role);
                    }
                }
                item
            })
            .collect()
    }
}

/// Normalized channel message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedChannelMessage {
    pub id: String,
    pub from: String,
    pub content: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

/// Nested JSON inside ChannelMessageRaw.payload.
#[derive(Debug, Clone, Deserialize)]
struct ChannelPayload {
    #[serde(default)]
    content: ChannelContent,
    #[serde(default)]
    metadata: ChannelMetadata,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ChannelContent {
    #[serde(default)]
    text: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ChannelMetadata {
    #[serde(default)]
    role: String,
}

// ── Chat Message Types ───────────────────────────────────────────────

/// Chat message content variants.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_suggestion: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_suggestion: Option<String>,
    #[serde(default)]
    pub install_confirmed: bool,
}

// ── Event Types ──────────────────────────────────────────────────────

/// Install event from NeboLoop (tool installed/uninstalled/revoked).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallEvent {
    pub event_type: String,
    pub app_id: String,
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// Task submission from another agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSubmission {
    pub task_id: String,
    pub from: String,
    pub description: String,
    #[serde(default)]
    pub input: serde_json::Value,
}

/// Task result sent back to requester.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskResult {
    pub task_id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Direct message from another bot or person.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectMessage {
    pub from: String,
    pub content: String,
    #[serde(default)]
    pub peer_type: String,
}

/// Channel message from a loop channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelMessage {
    pub channel_id: String,
    pub from: String,
    pub content: String,
}
