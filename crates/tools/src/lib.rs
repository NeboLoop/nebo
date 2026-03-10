pub mod app_tool;
pub mod bot_tool;
pub mod desktop_tool;
pub mod domain;
pub mod emit_tool;
pub mod execute_tool;
pub mod event_tool;
pub mod events;
pub mod loop_tool;
pub mod file_tool;
pub mod grep_tool;
pub mod keychain_tool;
pub mod message_tool;
pub mod music_tool;
pub mod orchestrator;
pub mod organizer_tool;
pub mod origin;
pub mod os_tool;
pub mod policy;
pub mod process;
pub mod registry;
pub mod role_tool;
pub mod safeguard;
pub mod settings_tool;
pub mod shell_tool;
pub mod skill_tool;
pub mod skills;
pub mod spotlight_tool;
pub mod system_tool;
pub mod sandbox_policy;
pub mod web_tool;
pub mod workflows;

pub use domain::*;
pub use file_tool::FileTool;
pub use orchestrator::{
    new_handle, OrchestratorHandle, SpawnRequest, SpawnResult, SubAgentOrchestrator,
};
pub use origin::{Origin, ToolContext};
pub use policy::{AskMode, Policy, PolicyLevel};
pub use process::ProcessRegistry;
pub use registry::{Registry, ToolResult};
pub use shell_tool::ShellTool;
pub use system_tool::SystemTool;
pub use os_tool::OsTool;
pub use role_tool::{RoleTool, ActiveRoleState};
pub use web_tool::WebTool;
pub use bot_tool::{AdvisorDeliberator, AgentTool, HybridSearchResult, HybridSearcher};
pub use event_tool::EventTool;
pub use skill_tool::SkillTool;
pub use message_tool::MessageTool;
pub use workflows::{WorkflowManager, WorkflowInfo, WorkflowRunInfo, WorkTool};
pub use events::{Event, EventBus};
pub use emit_tool::EmitTool;
pub use execute_tool::ExecuteTool;
pub use app_tool::AppTool;
pub use keychain_tool::KeychainTool;
pub use loop_tool::LoopTool;
pub use music_tool::MusicTool;
pub use organizer_tool::OrganizerTool;

/// Build a NeboLoop API client from a Store (for tool install actions).
pub(crate) fn build_neboloop_api(store: &db::Store) -> Result<comm::api::NeboLoopApi, String> {
    let bot_id = config::read_bot_id()
        .ok_or_else(|| "no bot_id configured — connect to NeboLoop first".to_string())?;
    let profiles = store
        .list_active_auth_profiles_by_provider("neboloop")
        .map_err(|e| format!("failed to query auth profiles: {}", e))?;
    let profile = profiles
        .first()
        .ok_or_else(|| "not connected to NeboLoop — redeem a NEBO code first".to_string())?;
    let cfg = config::Config::default();
    Ok(comm::api::NeboLoopApi::new(cfg.neboloop.api_url, bot_id, profile.api_key.clone()))
}

// ── Post-Install Artifact Persistence ──────────────────────────────
//
// After redeem_code() registers the install in NeboLoop, these fetch
// the actual content and write it to the local filesystem.

/// Extract the manifest text (SKILL.md/WORKFLOW.md/ROLE.md) from a SkillDetail.
/// Tries `manifest` field first, then falls back to `content_md`.
pub fn extract_manifest_text(detail: &comm::api_types::SkillDetail) -> Option<String> {
    // Primary: manifest field (can be JSON string or object)
    if let Some(ref v) = detail.manifest {
        let text = match v {
            serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
            serde_json::Value::Null => None,
            serde_json::Value::String(_) => None, // empty string
            _ => serde_json::to_string(v).ok(),
        };
        if text.is_some() {
            return text;
        }
    }
    // Fallback: content_md field
    detail.content_md.as_ref().filter(|s| !s.is_empty()).cloned()
}

/// Fetch skill content from NeboLoop and persist SKILL.md + manifest.json to nebo/ namespace.
/// Returns the skill directory path on success (for cascade dependency resolution).
pub async fn persist_skill_from_api(
    api: &comm::api::NeboLoopApi,
    artifact_id: &str,
    name: &str,
    code: &str,
) -> Result<std::path::PathBuf, String> {
    let detail = api.get_skill(artifact_id).await
        .map_err(|e| format!("fetch skill detail: {e}"))?;

    let has_api_manifest = extract_manifest_text(&detail).is_some();
    // Use manifest from API, or generate a minimal SKILL.md from metadata
    let manifest_text = extract_manifest_text(&detail)
        .unwrap_or_else(|| {
            tracing::info!(skill = name, "API returned no manifest; generating from metadata");
            generate_minimal_skill_md(name, &detail.item.description)
        });
    if has_api_manifest {
        tracing::debug!(skill = name, len = manifest_text.len(), "using manifest from API");
    }

    // Marketplace artifacts go to nebo/ namespace (installed), not user/
    let nebo_dir = config::nebo_dir().map_err(|e| format!("nebo_dir: {e}"))?;
    let slug = &detail.item.slug;
    let dir_name = if slug.is_empty() { name } else { slug.as_str() };
    let skill_dir = nebo_dir.join("skills").join(dir_name);

    std::fs::create_dir_all(&skill_dir).map_err(|e| format!("create skill dir: {e}"))?;
    std::fs::write(skill_dir.join("SKILL.md"), &manifest_text)
        .map_err(|e| format!("write SKILL.md: {e}"))?;

    let manifest_json = serde_json::json!({
        "name": name,
        "version": detail.item.version,
        "type": "skill",
        "code": code,
        "description": detail.item.description,
    });
    if let Err(e) = std::fs::write(
        skill_dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest_json).unwrap_or_default(),
    ) {
        tracing::warn!(skill = name, error = %e, "failed to write manifest.json");
    }

    tracing::info!(skill = name, dir = %skill_dir.display(), "persisted skill artifact");
    Ok(skill_dir)
}

/// Generate a minimal SKILL.md from metadata when the API returns no manifest content.
fn generate_minimal_skill_md(name: &str, description: &str) -> String {
    format!(
        "---\nname: {}\ndescription: {}\n---\n{}\n",
        name,
        if description.is_empty() { name } else { description },
        if description.is_empty() { "" } else { description },
    )
}

/// Fetch role content from NeboLoop and persist to DB + nebo/ namespace.
pub async fn persist_role_from_api(
    api: &comm::api::NeboLoopApi,
    artifact_id: &str,
    name: &str,
    code: &str,
    store: &db::Store,
) -> Result<(), String> {
    let detail = api.get_skill(artifact_id).await
        .map_err(|e| format!("fetch role detail: {e}"))?;

    let manifest_text = extract_manifest_text(&detail)
        .unwrap_or_else(|| generate_minimal_role_md(name, &detail.item.description));

    // Persist to DB
    let _ = store.create_role(
        artifact_id,
        Some(code),
        name,
        &detail.item.description,
        &manifest_text,
        "",
        None,
        None,
    ).map_err(|e| format!("create_role: {e}"))?;

    // Marketplace artifacts go to nebo/ namespace (installed)
    let nebo_dir = config::nebo_dir().map_err(|e| format!("nebo_dir: {e}"))?;
    let slug = &detail.item.slug;
    let dir_name = if slug.is_empty() { name } else { slug.as_str() };
    let role_dir = nebo_dir.join("roles").join(dir_name);
    std::fs::create_dir_all(&role_dir).map_err(|e| format!("create role dir: {e}"))?;

    if let Err(e) = std::fs::write(role_dir.join("ROLE.md"), &manifest_text) {
        tracing::warn!(role = name, error = %e, "failed to write ROLE.md");
    }

    let manifest_json = serde_json::json!({
        "name": name,
        "version": detail.item.version,
        "type": "role",
        "code": code,
        "description": detail.item.description,
    });
    if let Err(e) = std::fs::write(
        role_dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest_json).unwrap_or_default(),
    ) {
        tracing::warn!(role = name, error = %e, "failed to write manifest.json");
    }

    tracing::info!(role = name, dir = %role_dir.display(), "persisted role artifact");
    Ok(())
}

/// Generate a minimal ROLE.md from metadata.
fn generate_minimal_role_md(name: &str, description: &str) -> String {
    format!(
        "---\nname: {}\ndescription: {}\n---\n{}\n",
        name,
        if description.is_empty() { name } else { description },
        if description.is_empty() { "" } else { description },
    )
}
