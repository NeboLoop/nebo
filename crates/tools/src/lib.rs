#![recursion_limit = "256"]

pub mod a2ui_tool;
pub mod agent_tool;
pub mod app_tool;
pub mod bot_tool;
pub mod capabilities;
pub mod channel_bridge;
pub mod deep_research;
pub mod desktop_daemon;
pub mod desktop_snapshot;
pub mod desktop_tool;
pub mod domain;
pub mod emit_tool;
pub mod errors;
pub mod event_tool;
pub mod events;
pub mod execute_tool;
pub mod exit_tool;
pub mod file_tool;
pub mod grep_tool;
pub mod keychain_tool;
pub mod loop_tool;
pub mod mcp_tool;
pub mod memory_guard;
pub mod message_tool;
pub mod music_tool;
pub mod notebook_tool;
pub mod orchestrator;
mod organizer;
pub mod origin;
pub mod os_tool;
pub mod plugin_tool;
pub mod policy;
pub mod process;
pub mod publisher_tool;
pub mod registry;
pub mod research;
pub mod run_querier;
pub mod safeguard;
pub mod sandbox_policy;
pub mod settings_tool;
pub mod shell_tool;
pub mod sidecar_tool;
pub mod skill_tool;
pub mod skills;
pub mod spotlight_tool;
pub mod tool_search;
pub mod vm_tool;
pub mod web_tool;
pub mod workflows;

/// True when this Nebo runs as a cloud/container server rather than on a user's
/// desktop.
///
/// Desktop-bound resources (windows, input, screen capture, Mail/Calendar) have
/// no counterpart here: there is no display, window manager, or PIM app to talk
/// to. Their platform layers would otherwise fail deep inside with a cryptic
/// xdotool/Evolution error, so callers use this to refuse with a clear reason.
///
/// `NEBO_SERVER_MODE` forces it; otherwise a Linux box with neither `DISPLAY`
/// nor `WAYLAND_DISPLAY` is headless by definition (a Linux desktop always has
/// one). macOS/Windows are always treated as desktops.
pub fn server_mode() -> bool {
    if std::env::var_os("NEBO_SERVER_MODE").is_some() {
        return true;
    }
    cfg!(target_os = "linux")
        && std::env::var_os("DISPLAY").is_none()
        && std::env::var_os("WAYLAND_DISPLAY").is_none()
}

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

pub use a2ui_tool::{A2UIDomainTool, A2UIHost};
pub use agent_tool::{
    ActiveAgent, ActiveAgentState, AgentRegistry, PersonaTool, validate_agent_dependencies,
};
pub use app_tool::AppTool;
pub use bot_tool::{
    AdvisorDeliberator, AgentTool, CodeInstaller, HybridSearchResult, HybridSearcher,
    MemoryEmbedder,
};
pub use channel_bridge::{
    ChannelBridgeHandle, ChannelBridgeRegistry, OpResult, PendingOps, channel_bridge_key,
    channel_bridges, new_channel_bridge_registry, new_pending_ops, set_channel_bridges,
};
pub use domain::*;
pub use emit_tool::EmitTool;
pub use event_tool::EventTool;
pub use events::{Event, EventBus};
pub use execute_tool::ExecuteTool;
pub use exit_tool::{EXIT_SENTINEL, ExitTool};
pub use file_tool::FileTool;
pub use keychain_tool::KeychainTool;
pub use loop_tool::LoopTool;
pub use message_tool::MessageTool;
pub use music_tool::MusicTool;
pub use orchestrator::{
    OrchestratorHandle, SpawnRequest, SpawnResult, SubAgentOrchestrator, new_handle,
};
pub use origin::{
    ApprovalChannels, AskChannels, ChannelContext, ExecutionMode, Origin, ToolContext,
    workflow_session_key,
};
pub use os_tool::OsTool;
pub use policy::{AskMode, Policy, PolicyLevel};
pub use process::ProcessRegistry;
pub use registry::{Registry, ResourceKind, ToolResult};
pub use shell_tool::ShellTool;
pub use skill_tool::SkillTool;
pub use tool_search::ToolSearchTool;
pub use web_tool::WebTool;
pub use workflows::{WorkTool, WorkflowInfo, WorkflowManager, WorkflowRunInfo};

/// Build a NeboAI API client from a Store (for tool install actions).
pub(crate) fn build_neboai_api(store: &db::Store) -> Result<comm::api::NeboAIApi, String> {
    let bot_id = config::read_bot_id()
        .ok_or_else(|| "no bot_id configured — connect to NeboAI first".to_string())?;
    let profiles = store
        .list_active_auth_profiles_by_provider("neboai")
        .map_err(|e| format!("failed to query auth profiles: {}", e))?;
    let profile = profiles
        .first()
        .ok_or_else(|| "not connected to NeboAI — redeem a NEBO code first".to_string())?;
    let cfg = config::Config::default();
    Ok(comm::api::NeboAIApi::new(
        cfg.neboai.api_url,
        bot_id,
        profile.api_key.clone(),
    ))
}

// ── Post-Install Artifact Persistence ──────────────────────────────
//
// After redeem_code() registers the install in NeboAI, these fetch
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
    detail
        .content_md
        .as_ref()
        .filter(|s| !s.is_empty())
        .cloned()
}

/// Fetch skill content from NeboAI and persist to nebo/ namespace.
///
/// If the API provides a `downloadUrl`, downloads the sealed `.napp` archive
/// and stores it at `nebo/skills/{slug}/{version}.napp`, then extracts it.
/// Otherwise falls back to writing loose SKILL.md + manifest.json files.
///
/// Returns the skill directory path on success (for cascade dependency resolution).
pub async fn persist_skill_from_api(
    api: &comm::api::NeboAIApi,
    artifact_id: &str,
    name: &str,
    code: &str,
    store: Option<&db::Store>,
) -> Result<std::path::PathBuf, String> {
    let detail = api
        .get_skill(artifact_id)
        .await
        .map_err(|e| format!("fetch skill detail: {e}"))?;

    let nebo_dir = config::nebo_dir().map_err(|e| format!("nebo_dir: {e}"))?;
    let slug = &detail.item.slug;
    let dir_name = if slug.is_empty() { name } else { slug.as_str() };
    let version = if detail.item.version.is_empty() {
        "1.0.0"
    } else {
        &detail.item.version
    };

    // Skills can bundle a per-platform binary (bin/<name>), exactly like a
    // plugin or a sidecar app. The per-platform .napp carries that binary; the
    // generic `/download` serves the universal (binary-less) package. Prefer the
    // per-platform URL and fall back to the resolved generic URL for binary-less
    // skills (which 404 on the per-platform path).
    let platform = napp::plugin::current_platform_key();
    let platform_url = format!("/api/v1/apps/{}/download/{}.napp", artifact_id, platform);
    let download_candidates: Vec<String> = detail
        .download_url
        .iter()
        .cloned()
        .fold(vec![platform_url], |mut acc, u| {
            if !acc.contains(&u) {
                acc.push(u);
            }
            acc
        });
    if !download_candidates.is_empty() {
        let napp_dir = nebo_dir.join("skills").join(dir_name);
        std::fs::create_dir_all(&napp_dir).map_err(|e| format!("create skill dir: {e}"))?;
        let napp_path = napp_dir.join(format!("{}.napp", version));

        let mut downloaded: Option<Vec<u8>> = None;
        for candidate in &download_candidates {
            match api.download_napp(candidate).await {
                Ok(data) => {
                    downloaded = Some(data);
                    break;
                }
                Err(e) => {
                    tracing::debug!(skill = name, url = %candidate, error = %e, "napp download candidate failed, trying next");
                }
            }
        }
        match downloaded.ok_or_else(|| "all napp download candidates failed".to_string()) {
            Ok(data) => {
                std::fs::write(&napp_path, &data).map_err(|e| format!("write .napp: {e}"))?;
                tracing::info!(skill = name, path = %napp_path.display(), size = data.len(), "stored .napp");

                if napp::reader::is_sealed_napp(&napp_path) {
                    // Sealed (paid) skill — keep it sealed; the loader reads SKILL.md in
                    // memory via the license key. Seed the key now and partial-extract
                    // metadata (manifest.json — not the IP) so the loader resolves the
                    // artifact_id. Do NOT write a loose SKILL.md fallback: a sibling
                    // SKILL.md would make the loader treat this as free content.
                    tracing::info!(skill = name, "sealed skill — keeping .napp sealed; seeding license key");
                    if let Some(store) = store {
                        match fetch_and_store_license_keys(
                            api,
                            store,
                            &[artifact_id.to_string()],
                            "skill",
                        )
                        .await
                        {
                            Ok(keys) => {
                                if let Some(key) = keys.get(artifact_id) {
                                    if let Err(e) =
                                        napp::reader::partial_extract_sealed_napp(&napp_path, key)
                                    {
                                        tracing::warn!(skill = name, error = %e, "failed to partial-extract sealed skill metadata");
                                    }
                                } else {
                                    tracing::warn!(skill = name, "no license key returned for sealed skill — it will not load until keys refresh");
                                }
                            }
                            Err(e) => {
                                tracing::warn!(skill = name, error = %e, "failed to seed license key for sealed skill");
                            }
                        }
                    } else {
                        tracing::warn!(skill = name, "no store available to seed sealed skill license key");
                    }
                    // Sibling dir now holds the partial-extracted manifest.json.
                    return Ok(napp_path.with_extension(""));
                }

                // Free skill — extract alongside so the skill loader can find SKILL.md.
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
    let manifest_text = extract_manifest_text(&detail).unwrap_or_else(|| {
        tracing::info!(
            skill = name,
            "API returned no manifest; generating from metadata"
        );
        generate_minimal_skill_md(name, &detail.item.description)
    });
    if has_api_manifest {
        tracing::debug!(
            skill = name,
            len = manifest_text.len(),
            "using manifest from API"
        );
    }

    std::fs::create_dir_all(&skill_dir).map_err(|e| format!("create skill dir: {e}"))?;
    std::fs::write(skill_dir.join("SKILL.md"), &manifest_text)
        .map_err(|e| format!("write SKILL.md: {e}"))?;

    let manifest_json = serde_json::json!({
        "id": artifact_id,
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
        if description.is_empty() {
            name
        } else {
            description
        },
        if description.is_empty() {
            ""
        } else {
            description
        },
    )
}

/// Fetch agent content from NeboAI and persist to DB + nebo/ namespace.
///
/// If the API provides a `downloadUrl`, downloads the sealed `.napp` archive
/// and stores it at `nebo/agents/{slug}/{version}.napp`, then extracts it.
/// Otherwise falls back to writing loose AGENT.md + manifest.json files.
/// Result of persisting an agent from the API, including type_config for
/// downstream workflow binding processing.
pub struct PersistAgentResult {
    /// The typeConfig JSON from NeboAI (contains workflow bindings, triggers, etc.)
    pub type_config: Option<serde_json::Value>,
}

/// Fetch license keys for sealed artifacts from NeboAI and store them
/// (encrypted) in the local cache. Returns the decoded raw keys by artifact_id.
///
/// This is the single pathway for seeding and refreshing license keys: it is
/// called at install time to seed a freshly installed sealed artifact (so the
/// loader can decrypt it in memory) and by the periodic refresh to renew keys
/// before their TTL expires. The returned raw keys let the install path
/// partial-extract metadata (e.g. manifest.json) immediately.
pub async fn fetch_and_store_license_keys(
    api: &comm::api::NeboAIApi,
    store: &db::Store,
    artifact_ids: &[String],
    artifact_type: &str,
) -> Result<std::collections::HashMap<String, [u8; 32]>, String> {
    use base64::Engine;

    let mut keys = std::collections::HashMap::new();
    if artifact_ids.is_empty() {
        return Ok(keys);
    }

    let response = api
        .fetch_license_keys(artifact_ids)
        .await
        .map_err(|e| format!("fetch license keys: {e}"))?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    for (artifact_id, entry) in &response.keys {
        let key_bytes = base64::engine::general_purpose::STANDARD
            .decode(&entry.key)
            .map_err(|e| format!("base64 decode: {e}"))?;
        let Ok(key_arr): Result<[u8; 32], _> = key_bytes.try_into() else {
            tracing::warn!(artifact_id, "invalid license key length");
            continue;
        };
        // Encrypt with the keyring master key before storing at rest.
        let encrypted = auth::credential::encrypt(&entry.key)
            .map_err(|e| format!("encrypt key: {e}"))?;
        let expires_at = (now + entry.ttl) as i64;
        if let Err(e) =
            store.upsert_license_key(artifact_id, artifact_type, "user", &encrypted, expires_at)
        {
            tracing::warn!(artifact_id, error = %e, "failed to store license key");
        } else {
            keys.insert(artifact_id.clone(), key_arr);
        }
    }
    Ok(keys)
}

pub async fn persist_agent_from_api(
    api: &comm::api::NeboAIApi,
    artifact_id: &str,
    name: &str,
    code: &str,
    store: &db::Store,
) -> Result<PersistAgentResult, String> {
    // Try agent-specific endpoint first (GET /api/v1/agents/{slug}),
    // fall back to skill endpoint (GET /api/v1/skills/{id}) for older NeboAI versions.
    let derived_slug = name.to_lowercase().replace(' ', "-");

    let (
        manifest_text,
        frontmatter_str,
        description,
        agent_slug,
        version,
        download_url,
        type_config,
    ) = if let Ok(detail) = api.get_agent(&derived_slug).await {
        tracing::info!(agent = name, slug = %derived_slug, "fetched agent detail via /agents endpoint");
        let md = detail
            .content_md
            .clone()
            .unwrap_or_else(|| generate_minimal_agent_md(name, &detail.description));
        let fm = detail
            .type_config
            .as_ref()
            .map(|tc| serde_json::to_string(tc).unwrap_or_default())
            .unwrap_or_default();
        let slug = if detail.slug.is_empty() {
            derived_slug.clone()
        } else {
            detail.slug.clone()
        };
        let ver = if detail.version.is_empty() {
            "1.0.0".to_string()
        } else {
            detail.version.clone()
        };
        // downloadUrl is the API's package contract: present exactly when a
        // packaged .napp blob exists, absent for content-only agents. Do NOT
        // fabricate a fallback URL here — for content-only agents the generic
        // download endpoint 404s "no downloadable package", which used to fail
        // the whole install even though the full agent content was already in
        // this manifest response.
        let dl = detail.download_url.clone();
        (
            md,
            fm,
            detail.description.clone(),
            slug,
            ver,
            dl,
            detail.type_config,
        )
    } else {
        tracing::info!(
            agent = name,
            "agent endpoint unavailable, falling back to /skills endpoint"
        );
        let detail = api
            .get_skill(artifact_id)
            .await
            .map_err(|e| format!("fetch agent detail: {e}"))?;
        let md = extract_manifest_text(&detail)
            .unwrap_or_else(|| generate_minimal_agent_md(name, &detail.item.description));
        let fm = detail
            .type_config
            .as_ref()
            .map(|tc| serde_json::to_string(tc).unwrap_or_default())
            .unwrap_or_default();
        let slug = if detail.item.slug.is_empty() {
            derived_slug.clone()
        } else {
            detail.item.slug.clone()
        };
        let ver = if detail.item.version.is_empty() {
            "1.0.0".to_string()
        } else {
            detail.item.version.clone()
        };
        // Same contract as the /agents branch above: absent downloadUrl means
        // content-only — materialize from the manifest instead of fabricating a
        // download URL that 404s.
        let dl = detail.download_url.clone();
        (
            md,
            fm,
            detail.item.description.clone(),
            slug,
            ver,
            dl,
            detail.type_config,
        )
    };

    // Marketplace artifacts go to nebo/ namespace (installed). ALL filesystem
    // work happens in a staging dir OUTSIDE the loader's scan roots; the final
    // agents/<slug>/ directory only ever appears — via one rename — once the
    // payload is complete and validated. A failed install leaves nothing behind:
    // no empty agent dir that reads as "installed" in the marketplace, nothing
    // for the boot sync to trip over, nothing blocking a reinstall.
    let nebo_dir = config::nebo_dir().map_err(|e| format!("nebo_dir: {e}"))?;
    let staged = StagedInstall::begin(&nebo_dir, &agent_slug)?;
    let staged_version_dir = staged.path().join(&version);

    // The API advertises `downloadUrl` exactly when a packaged .napp exists
    // (sealed paid agents, apps with binaries, file-published agents). Agents
    // published content-only — AGENT.md persona + agent.json typeConfig in the
    // manifest, no package blob — have NO downloadUrl, and the generic download
    // endpoint 404s "no downloadable package" for them. For those, materialize
    // the loose files the loader reads from the manifest content we already
    // fetched, instead of failing a download that can never succeed.
    // (An empty string and a "/{platform}" URL template both mean "use the
    // per-platform candidate below", not a literal fetchable URL.)
    let download_url = download_url.filter(|u| !u.is_empty());

    let mut sealed = false;
    if let Some(ref generic_url) = download_url {
        // Download the .napp (always wrapped in a signed NAPP envelope).
        //
        // Free agents carry a plain tar.gz payload: extract it to loose files so the
        // loader can read AGENT.md/agent.json from disk. Sealed (paid) agents carry an
        // encrypted payload: leave the .napp sealed on disk — the loader decrypts it in
        // memory using the license key — and seed that license key now so the agent
        // can load immediately after install.
        // Apps are agents with a UI AND a native sidecar binary, so their package is
        // per-platform (bin/<name> + AGENT.md + agent.json), exactly like a plugin.
        // The generic `/download` endpoint serves the universal (UI-only / binary-less)
        // .napp, which for a sidecar app is missing the binary and the agent files —
        // that's why app installs landed incomplete. Prefer the per-platform URL and
        // fall back to the resolved generic URL for binary-less agents (which 404 on
        // the per-platform path).
        let platform_url = format!(
            "/api/v1/apps/{}/download/{}.napp",
            artifact_id,
            napp::plugin::current_platform_key()
        );
        let mut download_candidates: Vec<String> = vec![platform_url];
        if !generic_url.contains("{platform}") && !download_candidates.contains(generic_url) {
            download_candidates.push(generic_url.clone());
        }

        let napp_path = staged.path().join(format!("{}.napp", version));
        let mut downloaded: Option<Vec<u8>> = None;
        let mut last_err = String::new();
        for candidate in &download_candidates {
            match api.download_napp(candidate).await {
                Ok(data) => {
                    downloaded = Some(data);
                    break;
                }
                Err(e) => {
                    tracing::debug!(agent = name, url = %candidate, error = %e, "napp download candidate failed, trying next");
                    last_err = e.to_string();
                }
            }
        }
        // A failed download of an advertised package is an install FAILURE —
        // propagate the real error so the UI shows it (staging is dropped clean).
        let data =
            downloaded.ok_or_else(|| format!("download agent package for {name}: {last_err}"))?;
        std::fs::write(&napp_path, &data).map_err(|e| format!("write .napp: {e}"))?;
        tracing::info!(agent = name, path = %napp_path.display(), size = data.len(), "stored .napp");

        sealed = napp::reader::is_sealed_napp(&napp_path);
        if sealed {
            tracing::info!(agent = name, "sealed agent — keeping .napp sealed; seeding license key");
            match fetch_and_store_license_keys(api, store, &[artifact_id.to_string()], "agent")
                .await
            {
                Ok(keys) => {
                    // Partial-extract metadata (manifest.json) — not the IP — so the
                    // loader can resolve the artifact_id and match the license key.
                    if let Some(key) = keys.get(artifact_id) {
                        if let Err(e) = napp::reader::partial_extract_sealed_napp(&napp_path, key)
                        {
                            tracing::warn!(agent = name, error = %e, "failed to partial-extract sealed agent metadata");
                        }
                    } else {
                        tracing::warn!(agent = name, "no license key returned for sealed agent — it will not load until keys refresh");
                    }
                }
                Err(e) => {
                    tracing::warn!(agent = name, error = %e, "failed to seed license key for sealed agent");
                }
            }
        } else {
            let extract_dir = napp::reader::extract_napp_alongside(&napp_path)
                .map_err(|e| format!("extract agent package for {name}: {e}"))?;
            tracing::info!(agent = name, dir = %extract_dir.display(), "extracted .napp");
        }
    } else {
        // Content-only agent: materialize the loose files the loader reads —
        // the SAME layout an extracted .napp produces (version dir with
        // AGENT.md + agent.json + manifest.json carrying the artifact id the
        // boot fs→DB sync matches on).
        std::fs::create_dir_all(&staged_version_dir)
            .map_err(|e| format!("create agent version dir: {e}"))?;
        std::fs::write(staged_version_dir.join("AGENT.md"), &manifest_text)
            .map_err(|e| format!("write AGENT.md: {e}"))?;
        if !frontmatter_str.is_empty() {
            std::fs::write(staged_version_dir.join("agent.json"), &frontmatter_str)
                .map_err(|e| format!("write agent.json: {e}"))?;
        }
        let manifest = serde_json::json!({
            "id": artifact_id,
            "name": name,
            "slug": agent_slug,
            "version": version,
            "type": "agent",
        });
        std::fs::write(
            staged_version_dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap_or_default(),
        )
        .map_err(|e| format!("write manifest.json: {e}"))?;
        tracing::info!(agent = name, "materialized content-only agent from manifest (no package blob)");
    }

    // Validate the staged payload BEFORE it can reach the final location.
    // Free agents: AGENT.md is the loader's discovery marker — without it the
    // agent can never load. Sealed agents have no loose files (the IP stays
    // encrypted) and are validated by the loader reading them in memory with
    // the seeded license key.
    if !sealed && !staged_version_dir.join("AGENT.md").exists() {
        tracing::error!(
            agent = name,
            "agent package is incomplete — missing AGENT.md"
        );
        return Err(format!(
            "agent package for {name} is incomplete: missing AGENT.md"
        ));
    }

    // Atomically publish: the whole slug dir is swapped, which also replaces any
    // older version dirs/.napp files wholesale (the loader walks every AGENT.md
    // and last-writes by name, so stale sibling versions must not survive).
    let version_dir = staged.commit()?.join(&version);

    // Persist to DB — create or update if already exists (re-install). After the
    // filesystem, matching the system's source-of-truth order: the boot fs→DB
    // sync recreates a missing row from disk, but nothing recreates missing disk
    // content from a row.
    if store.get_agent(artifact_id).ok().flatten().is_some() {
        let _ = store.update_agent(
            artifact_id,
            name,
            &description,
            &manifest_text,
            &frontmatter_str,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
    } else {
        let _ = store
            .create_agent(
                artifact_id,
                Some(code),
                name,
                &description,
                &manifest_text,
                &frontmatter_str,
                None,
                None,
            )
            .map_err(|e| format!("create_agent: {e}"))?;
    }

    tracing::info!(agent = name, dir = %version_dir.display(), "persisted agent artifact");
    Ok(PersistAgentResult { type_config })
}

/// Staged atomic install of a marketplace agent's filesystem payload.
///
/// All payload work (download, extraction, materialization) happens in a
/// unique directory under `<nebo_dir>/.staging/` — outside every loader's scan
/// root — and `agents/<slug>/` only ever appears via [`StagedInstall::commit`]'s
/// rename once the payload is complete. Dropping without committing (any error
/// path) removes the staging dir, so a failed install leaves no trace.
pub struct StagedInstall {
    staging: std::path::PathBuf,
    final_dir: std::path::PathBuf,
    committed: bool,
}

impl StagedInstall {
    /// Begin a staged install targeting `<root>/agents/<slug>`.
    pub fn begin(root: &std::path::Path, slug: &str) -> Result<Self, String> {
        let staging = root.join(".staging").join(format!(
            "agent-{}-{}",
            slug,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&staging).map_err(|e| format!("create staging dir: {e}"))?;
        Ok(Self {
            staging,
            final_dir: root.join("agents").join(slug),
            committed: false,
        })
    }

    /// The staging directory to build the payload in.
    pub fn path(&self) -> &std::path::Path {
        &self.staging
    }

    /// Publish the staged payload: replaces any previous install wholesale by
    /// renaming the staging dir into place. Returns the final directory.
    pub fn commit(mut self) -> Result<std::path::PathBuf, String> {
        if let Some(parent) = self.final_dir.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create agents dir: {e}"))?;
        }
        if self.final_dir.exists() {
            std::fs::remove_dir_all(&self.final_dir)
                .map_err(|e| format!("replace previous install: {e}"))?;
        }
        std::fs::rename(&self.staging, &self.final_dir)
            .map_err(|e| format!("publish staged install: {e}"))?;
        self.committed = true;
        Ok(self.final_dir.clone())
    }
}

impl Drop for StagedInstall {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_dir_all(&self.staging);
        }
    }
}

#[cfg(test)]
mod staged_install_tests {
    use super::StagedInstall;

    // The failure contract: an install that dies mid-payload (download 404,
    // extraction error, validation failure) must leave NOTHING behind — no
    // agents/<slug>/ dir the marketplace would report as "installed", and no
    // staging debris.
    #[test]
    fn failure_leaves_nothing_behind() {
        let root = tempfile::tempdir().unwrap();
        let staged = StagedInstall::begin(root.path(), "sdr").unwrap();
        std::fs::write(staged.path().join("1.0.0.napp"), b"partial").unwrap();
        drop(staged); // simulated failure: dropped without commit

        assert!(
            !root.path().join("agents").join("sdr").exists(),
            "failed install must not create the final agent dir"
        );
        let staging_empty = std::fs::read_dir(root.path().join(".staging"))
            .map(|mut e| e.next().is_none())
            .unwrap_or(true);
        assert!(staging_empty, "failed install must not leave staging debris");
    }

    #[test]
    fn commit_materializes_content_and_replaces_previous_install() {
        let root = tempfile::tempdir().unwrap();
        // Previous install (e.g. an older version, or an empty dir from a failed
        // pre-atomic install) must be replaced wholesale.
        let final_dir = root.path().join("agents").join("sdr");
        std::fs::create_dir_all(final_dir.join("0.9.0")).unwrap();
        std::fs::write(final_dir.join("0.9.0.napp"), b"old").unwrap();

        let staged = StagedInstall::begin(root.path(), "sdr").unwrap();
        let vdir = staged.path().join("1.0.0");
        std::fs::create_dir_all(&vdir).unwrap();
        std::fs::write(vdir.join("AGENT.md"), "---\nname: Sdr\n---\n").unwrap();
        let out = staged.commit().unwrap();

        assert_eq!(out, final_dir);
        assert!(final_dir.join("1.0.0").join("AGENT.md").exists());
        assert!(
            !final_dir.join("0.9.0").exists() && !final_dir.join("0.9.0.napp").exists(),
            "previous install must be replaced wholesale (no stale versions)"
        );
    }
}

/// Generate a minimal AGENT.md from metadata.
fn generate_minimal_agent_md(name: &str, description: &str) -> String {
    format!(
        "---\nname: {}\ndescription: {}\n---\n{}\n",
        name,
        if description.is_empty() {
            name
        } else {
            description
        },
        if description.is_empty() {
            ""
        } else {
            description
        },
    )
}
