pub mod app_tool;
pub mod bot_tool;
pub mod desktop_daemon;
pub mod desktop_snapshot;
pub mod desktop_tool;
pub mod domain;
pub mod emit_tool;
pub mod execute_tool;
pub mod exit_tool;
pub mod event_tool;
pub mod events;
pub mod loop_tool;
pub mod file_tool;
pub mod grep_tool;
pub mod keychain_tool;
pub mod message_tool;
pub mod music_tool;
pub mod orchestrator;
mod organizer;
pub mod run_querier;
pub mod organizer_tool;
pub mod origin;
pub mod os_tool;
pub mod plugin_tool;
pub mod policy;
pub mod process;
pub mod publisher_tool;
pub mod registry;
pub mod agent_tool;
pub mod safeguard;
pub mod settings_tool;
pub mod shell_tool;
pub mod skill_tool;
pub mod skills;
pub mod spotlight_tool;
pub mod mcp_tool;
pub mod system_tool;
pub mod sandbox_policy;
pub mod web_tool;
pub mod workflows;

/// Truncate a string to at most `max_bytes` bytes without splitting a multi-byte
/// UTF-8 character.
pub fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

pub use domain::*;
pub use file_tool::FileTool;
pub use orchestrator::{
    new_handle, OrchestratorHandle, SpawnRequest, SpawnResult, SubAgentOrchestrator,
};
pub use origin::{Origin, ToolContext};
pub use policy::{AskMode, Policy, PolicyLevel};
pub use process::ProcessRegistry;
pub use registry::{Registry, ResourceKind, ToolResult};
pub use shell_tool::ShellTool;
pub use system_tool::SystemTool;
pub use os_tool::OsTool;
pub use agent_tool::{PersonaTool, ActiveAgentState, ActiveAgent, AgentRegistry, validate_agent_dependencies};
pub use web_tool::WebTool;
pub use bot_tool::{AdvisorDeliberator, AgentTool, HybridSearchResult, HybridSearcher};
pub use event_tool::EventTool;
pub use skill_tool::SkillTool;
pub use message_tool::MessageTool;
pub use workflows::{WorkflowManager, WorkflowInfo, WorkflowRunInfo, WorkTool};
pub use events::{Event, EventBus};
pub use emit_tool::EmitTool;
pub use execute_tool::ExecuteTool;
pub use exit_tool::{ExitTool, EXIT_SENTINEL};
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

/// Extract the manifest text (SKILL.md/WORKFLOW.md/AGENT.md) from a SkillDetail.
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

/// Fetch skill content from NeboLoop and persist to nebo/ namespace.
///
/// If the API provides a `downloadUrl`, downloads the sealed `.napp` archive
/// and stores it at `nebo/skills/{slug}/{version}.napp`, then extracts it.
/// Otherwise falls back to writing loose SKILL.md + manifest.json files.
///
/// Returns the skill directory path on success (for cascade dependency resolution).
pub async fn persist_skill_from_api(
    api: &comm::api::NeboLoopApi,
    artifact_id: &str,
    name: &str,
    code: &str,
) -> Result<std::path::PathBuf, String> {
    let detail = api.get_skill(artifact_id).await
        .map_err(|e| format!("fetch skill detail: {e}"))?;

    let nebo_dir = config::nebo_dir().map_err(|e| format!("nebo_dir: {e}"))?;
    let slug = &detail.item.slug;
    let dir_name = if slug.is_empty() { name } else { slug.as_str() };
    let version = if detail.item.version.is_empty() { "1.0.0" } else { &detail.item.version };

    // Try sealed .napp download — use API-provided URL or construct from artifact ID.
    // Include platform so the server can serve the right binary for this OS/arch.
    let platform = napp::plugin::current_platform_key();
    let download_url = detail.download_url.clone()
        .or_else(|| Some(format!("/api/v1/apps/{}/download/{}", artifact_id, platform)));
    if let Some(ref download_url) = download_url {
        let napp_dir = nebo_dir.join("skills").join(dir_name);
        std::fs::create_dir_all(&napp_dir).map_err(|e| format!("create skill dir: {e}"))?;
        let napp_path = napp_dir.join(format!("{}.napp", version));

        match api.download_napp(download_url).await {
            Ok(data) => {
                std::fs::write(&napp_path, &data)
                    .map_err(|e| format!("write .napp: {e}"))?;
                tracing::info!(skill = name, path = %napp_path.display(), size = data.len(), "stored sealed .napp");

                // Extract alongside so the skill loader can find SKILL.md
                // e.g. nebo/skills/my-cloud/1.0.0.napp → nebo/skills/my-cloud/1.0.0/
                match napp::reader::extract_napp_alongside(&napp_path) {
                    Ok(extract_dir) => {
                        tracing::info!(skill = name, dir = %extract_dir.display(), "extracted .napp");
                        return Ok(extract_dir);
                    }
                    Err(e) => {
                        tracing::warn!(skill = name, error = %e, "failed to extract .napp; falling back to loose files");
                    }
                }
            }
            Err(e) => {
                tracing::warn!(skill = name, error = %e, "failed to download .napp; falling back to loose files");
            }
        }
    }

    // Fallback: write loose SKILL.md + manifest.json
    let skill_dir = nebo_dir.join("skills").join(dir_name);
    let has_api_manifest = extract_manifest_text(&detail).is_some();
    let manifest_text = extract_manifest_text(&detail)
        .unwrap_or_else(|| {
            tracing::info!(skill = name, "API returned no manifest; generating from metadata");
            generate_minimal_skill_md(name, &detail.item.description)
        });
    if has_api_manifest {
        tracing::debug!(skill = name, len = manifest_text.len(), "using manifest from API");
    }

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

    tracing::info!(skill = name, dir = %skill_dir.display(), "persisted skill artifact (loose)");
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

/// Fetch agent content from NeboLoop and persist to DB + nebo/ namespace.
///
/// If the API provides a `downloadUrl`, downloads the sealed `.napp` archive
/// and stores it at `nebo/agents/{slug}/{version}.napp`, then extracts it.
/// Otherwise falls back to writing loose AGENT.md + manifest.json files.
/// Result of persisting an agent from the API, including type_config for
/// downstream workflow binding processing.
pub struct PersistAgentResult {
    /// The typeConfig JSON from NeboLoop (contains workflow bindings, triggers, etc.)
    pub type_config: Option<serde_json::Value>,
}

pub async fn persist_agent_from_api(
    api: &comm::api::NeboLoopApi,
    artifact_id: &str,
    name: &str,
    code: &str,
    store: &db::Store,
) -> Result<PersistAgentResult, String> {
    let detail = api.get_skill(artifact_id).await
        .map_err(|e| format!("fetch agent detail: {e}"))?;

    let manifest_text = extract_manifest_text(&detail)
        .unwrap_or_else(|| generate_minimal_agent_md(name, &detail.item.description));

    // Store typeConfig as frontmatter so workflow bindings are preserved
    let frontmatter_str = detail.type_config.as_ref()
        .map(|tc| serde_json::to_string(tc).unwrap_or_default())
        .unwrap_or_default();

    // Persist to DB — create or update if already exists (re-install)
    if store.get_agent(artifact_id).ok().flatten().is_some() {
        let _ = store.update_agent(
            artifact_id,
            name,
            &detail.item.description,
            &manifest_text,
            &frontmatter_str,
            None,
            None,
        );
    } else {
        let _ = store.create_agent(
            artifact_id,
            Some(code),
            name,
            &detail.item.description,
            &manifest_text,
            &frontmatter_str,
            None,
            None,
        ).map_err(|e| format!("create_agent: {e}"))?;
    }

    // Marketplace artifacts go to nebo/ namespace (installed)
    let nebo_dir = config::nebo_dir().map_err(|e| format!("nebo_dir: {e}"))?;
    let slug = &detail.item.slug;
    let dir_name = if slug.is_empty() { name } else { slug.as_str() };
    let version = if detail.item.version.is_empty() { "1.0.0" } else { &detail.item.version };

    // Try sealed .napp download — use API-provided URL or construct from artifact ID
    let download_url = detail.download_url.clone()
        .or_else(|| Some(format!("/api/v1/apps/{}/download", artifact_id)));
    if let Some(ref download_url) = download_url {
        let napp_dir = nebo_dir.join("agents").join(dir_name);
        std::fs::create_dir_all(&napp_dir).map_err(|e| format!("create agent dir: {e}"))?;
        let napp_path = napp_dir.join(format!("{}.napp", version));

        match api.download_napp(download_url).await {
            Ok(data) => {
                std::fs::write(&napp_path, &data)
                    .map_err(|e| format!("write .napp: {e}"))?;
                tracing::info!(agent = name, path = %napp_path.display(), size = data.len(), "stored sealed .napp");

                match napp::reader::extract_napp_alongside(&napp_path) {
                    Ok(extract_dir) => {
                        tracing::info!(agent = name, dir = %extract_dir.display(), "extracted .napp");
                        return Ok(PersistAgentResult { type_config: detail.type_config });
                    }
                    Err(e) => {
                        tracing::warn!(agent = name, error = %e, "failed to extract .napp; falling back to loose files");
                    }
                }
            }
            Err(e) => {
                tracing::warn!(agent = name, error = %e, "failed to download .napp; falling back to loose files");
            }
        }
    }

    // Fallback: write loose AGENT.md + agent.json + manifest.json
    let agent_dir = nebo_dir.join("agents").join(dir_name);
    std::fs::create_dir_all(&agent_dir).map_err(|e| format!("create agent dir: {e}"))?;

    if let Err(e) = std::fs::write(agent_dir.join("AGENT.md"), &manifest_text) {
        tracing::warn!(agent = name, error = %e, "failed to write AGENT.md");
    }

    // Write agent.json from typeConfig (contains workflow bindings, triggers)
    if let Some(ref tc) = detail.type_config {
        if let Err(e) = std::fs::write(
            agent_dir.join("agent.json"),
            serde_json::to_string_pretty(tc).unwrap_or_default(),
        ) {
            tracing::warn!(agent = name, error = %e, "failed to write agent.json");
        }
    }

    let manifest_json = serde_json::json!({
        "name": name,
        "version": detail.item.version,
        "type": "agent",
        "code": code,
        "description": detail.item.description,
    });
    if let Err(e) = std::fs::write(
        agent_dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest_json).unwrap_or_default(),
    ) {
        tracing::warn!(agent = name, error = %e, "failed to write manifest.json");
    }

    tracing::info!(agent = name, dir = %agent_dir.display(), "persisted agent artifact (loose)");
    Ok(PersistAgentResult { type_config: detail.type_config })
}

/// Generate a minimal AGENT.md from metadata.
fn generate_minimal_agent_md(name: &str, description: &str) -> String {
    format!(
        "---\nname: {}\ndescription: {}\n---\n{}\n",
        name,
        if description.is_empty() { name } else { description },
        if description.is_empty() { "" } else { description },
    )
}
