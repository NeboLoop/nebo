//! Dependency auto-install cascade resolver.
//!
//! Walks the STRAP hierarchy (AGENT → WORK → SKILL) downward,
//! checks local presence, and either auto-installs (autonomous mode)
//! or reports pending (non-autonomous mode).

use std::collections::HashSet;

use axum::extract::State;
use axum::response::Json;
use serde::{Deserialize, Serialize};

use comm::api::NeboLoopApi;

use crate::handlers::HandlerResult;
use crate::state::AppState;

// ── Types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DepType {
    Skill,
    Workflow,
    Plugin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepRef {
    #[serde(rename = "depType")]
    pub dep_type: DepType,
    pub reference: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DepStatus {
    AlreadyInstalled,
    Installed,
    PendingApproval,
    Failed { error: String },
    Unresolvable { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepResult {
    pub dep: DepRef,
    pub status: DepStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<DepResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CascadeResult {
    pub results: Vec<DepResult>,
    pub installed_count: usize,
    pub pending_count: usize,
    pub failed_count: usize,
}

// ── Core Resolver ───────────────────────────────────────────────────

/// Main entry — respects autonomous_mode from DB settings.
pub async fn resolve_cascade(
    state: &AppState,
    deps: Vec<DepRef>,
    visited: &mut HashSet<String>,
) -> CascadeResult {
    resolve_cascade_inner(state, deps, visited, false).await
}

/// Force-install variant — called when user explicitly approves pending deps.
pub async fn resolve_cascade_force(
    state: &AppState,
    deps: Vec<DepRef>,
    visited: &mut HashSet<String>,
) -> CascadeResult {
    resolve_cascade_inner(state, deps, visited, true).await
}

fn resolve_cascade_inner<'a>(
    state: &'a AppState,
    deps: Vec<DepRef>,
    visited: &'a mut HashSet<String>,
    force: bool,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = CascadeResult> + Send + 'a>> {
    Box::pin(async move {
    let autonomous = force || is_autonomous(state);
    let mut results = Vec::new();
    let mut installed_count = 0usize;
    let mut pending_count = 0usize;
    let mut failed_count = 0usize;

    for dep in deps {
        // Cycle / dedup check
        let key = format!("{:?}:{}", dep.dep_type, dep.reference);
        if !visited.insert(key) {
            continue;
        }

        // Check if already installed
        if is_installed(state, &dep).await {
            results.push(DepResult {
                dep,
                status: DepStatus::AlreadyInstalled,
                children: vec![],
            });
            continue;
        }

        // Simple names (no @ prefix, no install code) are built-in — mark unresolvable
        if !is_marketplace_ref(&dep.reference) {
            let reason = format!("'{}' is a built-in or simple name, not a marketplace ref", dep.reference);
            results.push(DepResult {
                dep,
                status: DepStatus::Unresolvable { reason },
                children: vec![],
            });
            continue;
        }

        if autonomous {
            match install_dep(state, &dep).await {
                Ok(child_deps) => {
                    state.hub.broadcast(
                        "dep_installed",
                        serde_json::json!({
                            "depType": format!("{:?}", dep.dep_type),
                            "reference": dep.reference,
                        }),
                    );
                    installed_count += 1;

                    // Recurse into child deps
                    let child_result = resolve_cascade_inner(state, child_deps, visited, force).await;
                    installed_count += child_result.installed_count;
                    pending_count += child_result.pending_count;
                    failed_count += child_result.failed_count;

                    results.push(DepResult {
                        dep,
                        status: DepStatus::Installed,
                        children: child_result.results,
                    });
                }
                Err(e) => {
                    state.hub.broadcast(
                        "dep_failed",
                        serde_json::json!({
                            "depType": format!("{:?}", dep.dep_type),
                            "reference": dep.reference,
                            "error": e,
                        }),
                    );
                    failed_count += 1;
                    results.push(DepResult {
                        dep,
                        status: DepStatus::Failed { error: e },
                        children: vec![],
                    });
                }
            }
        } else {
            state.hub.broadcast(
                "dep_pending",
                serde_json::json!({
                    "depType": format!("{:?}", dep.dep_type),
                    "reference": dep.reference,
                }),
            );
            pending_count += 1;
            results.push(DepResult {
                dep,
                status: DepStatus::PendingApproval,
                children: vec![],
            });
        }
    }

    state.hub.broadcast(
        "dep_cascade_complete",
        serde_json::json!({
            "installed": installed_count,
            "pending": pending_count,
            "failed": failed_count,
        }),
    );

    CascadeResult {
        results,
        installed_count,
        pending_count,
        failed_count,
    }
    }) // Box::pin
}

// ── Autonomy Check ──────────────────────────────────────────────────

fn is_autonomous(state: &AppState) -> bool {
    state
        .store
        .get_settings()
        .ok()
        .flatten()
        .map(|s| s.autonomous_mode == 1)
        .unwrap_or(false)
}

// ── Marketplace Ref Detection ───────────────────────────────────────

/// A marketplace ref is either a qualified name (@org/type/name) or an install code (PREFIX-XXXX-XXXX).
pub fn is_marketplace_ref(reference: &str) -> bool {
    reference.starts_with('@')
        || reference.starts_with("SKIL-")
        || reference.starts_with("WORK-")
        || reference.starts_with("AGNT-")
        || reference.starts_with("PLUG-")
}

// ── Presence Detection ──────────────────────────────────────────────

async fn is_installed(state: &AppState, dep: &DepRef) -> bool {
    match dep.dep_type {
        DepType::Skill => is_skill_installed(&dep.reference),
        DepType::Workflow => is_workflow_installed(state, &dep.reference),
        DepType::Plugin => state.plugin_store.resolve(&dep.reference, "*").is_some(),
    }
}

fn is_skill_installed(reference: &str) -> bool {
    let simple_name = extract_simple_name(reference);
    if let (Ok(user_dir), Ok(nebo_dir)) = (config::user_dir(), config::nebo_dir()) {
        let user_skills = user_dir.join("skills");
        let nebo_skills = nebo_dir.join("skills");

        // Check user dir: name/SKILL.md
        if user_skills.join(simple_name).join("SKILL.md").exists() {
            return true;
        }

        // Check nebo dir: look for extracted directories or .napp files
        if nebo_skills.exists() {
            // For qualified refs, check the specific path
            if reference.starts_with('@') {
                let ref_no_version = reference.split('@').take(2).collect::<Vec<_>>().join("@");
                let ref_path = ref_no_version.trim_start_matches('@');
                if nebo_dir.join(ref_path).exists() {
                    return true;
                }
            }
            // Check for extracted version directories containing SKILL.md
            if has_extracted_skill(&nebo_skills) {
                return true;
            }
            // Fallback: check for .napp files (pre-migration)
            if has_napp_files(&nebo_skills) {
                return true;
            }
        }
    }
    false
}

fn is_workflow_installed(state: &AppState, reference: &str) -> bool {
    // Check by install code
    if reference.starts_with("WORK-") {
        return state
            .store
            .get_workflow_by_code(reference)
            .ok()
            .flatten()
            .is_some();
    }

    // Check by name/ID search
    let simple_name = extract_simple_name(reference);
    if let Ok(workflows) = state.store.list_workflows(100, 0) {
        let lower = simple_name.to_lowercase();
        for wf in &workflows {
            if wf.name.to_lowercase() == lower || wf.id == reference {
                return true;
            }
        }
    }
    false
}

/// Extract the simple artifact name from a qualified ref or install code.
/// `@org/type/name@version` → `name`
/// `SKIL-XXXX-XXXX` → `SKIL-XXXX-XXXX` (unchanged)
/// `web` → `web`
pub fn extract_simple_name(reference: &str) -> &str {
    if reference.starts_with('@') {
        // Strip version suffix: @org/type/name@^1.0 → @org/type/name
        let no_version = if let Some(at_pos) = reference[1..].find('@') {
            &reference[..at_pos + 1]
        } else {
            reference
        };
        // Get last segment: @org/type/name → name
        no_version.rsplit('/').next().unwrap_or(reference)
    } else {
        reference
    }
}

/// Check if a directory tree contains any extracted skill directories (with SKILL.md).
fn has_extracted_skill(dir: &std::path::Path) -> bool {
    let mut found = false;
    napp::reader::walk_for_marker(dir, "SKILL.md", &mut |_| {
        found = true;
    });
    found
}

fn has_napp_files(dir: &std::path::Path) -> bool {
    fn walk(dir: &std::path::Path) -> bool {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return false,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if walk(&path) {
                    return true;
                }
            } else if path.extension().is_some_and(|ext| ext == "napp") {
                return true;
            }
        }
        false
    }
    walk(dir)
}

// ── Install Dispatch ────────────────────────────────────────────────

async fn install_dep(state: &AppState, dep: &DepRef) -> Result<Vec<DepRef>, String> {
    let api = build_api_client(state).map_err(|e| e.to_string())?;

    match dep.dep_type {
        DepType::Skill => install_skill(state, &api, &dep.reference).await,
        DepType::Workflow => install_workflow(state, &api, &dep.reference).await,
        DepType::Plugin => install_plugin(state, &api, &dep.reference).await,
    }
}

async fn install_skill(
    state: &AppState,
    api: &NeboLoopApi,
    reference: &str,
) -> Result<Vec<DepRef>, String> {
    // Redeem the code with NeboLoop to register the install
    let resp = api.install_skill(reference)
        .await
        .map_err(|e| format!("install_skill: {}", e))?;

    let artifact_id = resp.artifact.id.clone();
    let name = resp.artifact.name.clone();

    // Persist to disk (download .napp or write loose files)
    let skill_dir = match tools::persist_skill_from_api(api, &artifact_id, &name, reference).await {
        Ok(dir) => {
            tracing::info!(reference, name = %name, dir = %dir.display(), "cascade: persisted skill");
            Some(dir)
        }
        Err(e) => {
            tracing::warn!(reference, error = %e, "cascade: failed to persist skill");
            None
        }
    };

    // Reload skill loader so it appears immediately
    state.skill_loader.load_all().await;

    // Extract child deps from the newly installed skill
    if let Some(skill_dir) = skill_dir {
        let skill_path = skill_dir.join("SKILL.md");
        if let Ok(data) = std::fs::read(&skill_path) {
            if let Ok(skill) = tools::skills::parse_skill_md(&data) {
                return Ok(extract_skill_deps(&skill));
            }
        }
    }

    Ok(vec![])
}

async fn install_workflow(
    state: &AppState,
    api: &NeboLoopApi,
    reference: &str,
) -> Result<Vec<DepRef>, String> {
    let resp = api
        .install_workflow(reference)
        .await
        .map_err(|e| format!("install_workflow: {}", e))?;

    // Try to extract child deps from the newly-installed workflow
    if let Ok(Some(wf)) = state.store.get_workflow(&resp.artifact.id) {
        if let Ok(def) = workflow::parser::parse_workflow(&wf.definition) {
            return Ok(extract_workflow_deps(&def));
        }
    }

    Ok(vec![])
}

async fn install_plugin(
    state: &AppState,
    api: &NeboLoopApi,
    reference: &str,
) -> Result<Vec<DepRef>, String> {
    // Redeem the install code (plugins use the same redeem endpoint)
    let resp = api.install_skill(reference).await
        .map_err(|e| format!("redeem plugin code: {e}"))?;

    let name = resp.artifact.name.clone();
    let slug = if resp.artifact.slug.is_empty() {
        name.to_lowercase().replace(' ', "-")
    } else {
        resp.artifact.slug.clone()
    };
    let download_url = resp.download_url
        .ok_or_else(|| format!("plugin {} has no download URL in redeem response", name))?;

    // Download .napp directly from redeem response URL
    let napp_data = api.download_napp(&download_url).await
        .map_err(|e| format!("download .napp for {}: {}", slug, e))?;

    tracing::info!(plugin = %name, slug = %slug, size = napp_data.len(), "cascade: downloaded plugin .napp");

    // Pause skill watcher during extraction to prevent premature reloads
    state.skill_loader.pause_watcher();

    // Remove existing version AFTER download completes
    let _ = state.plugin_store.remove(&slug);

    // Install from .napp archive (extracts binary, plugin.json, embedded skills)
    let install_result = state.plugin_store.install_from_napp(&slug, "latest", &napp_data).await;

    match install_result {
        Ok(path) => {
            tracing::info!(plugin = %name, path = %path.display(), "cascade: installed plugin");

            // Reload skills (picks up embedded skills from plugin .napp)
            state.skill_loader.load_all().await;
            state.skill_loader.resume_watcher();

            // Re-register plugin tool
            state.tools.unregister("plugin").await;
            if !state.plugin_store.list_installed().is_empty() {
                state.tools.register(Box::new(
                    tools::plugin_tool::PluginTool::new(state.plugin_store.clone())
                )).await;
            }
        }
        Err(e) => {
            state.skill_loader.resume_watcher();
            return Err(format!("plugin install failed: {e}"));
        }
    }

    // Plugins don't have child dependencies
    Ok(vec![])
}

// ── Dep Extraction ──────────────────────────────────────────────────

/// Extract dependencies from an agent's frontmatter JSON string.
///
/// Workflows are now inline (no external refs). Only skill dependencies are extracted
/// from both the top-level `skills` array and from inline activity skill references.
pub fn extract_agent_deps_from_frontmatter(frontmatter_json: &str) -> Vec<DepRef> {
    let mut deps = Vec::new();
    // Try parsing as full AgentConfig first (has typed workflows with activities)
    match napp::agent::parse_agent_config(frontmatter_json) {
        Ok(config) => {
            tracing::info!(
                plugins = config.requires.plugins.len(),
                skills = config.skills.len(),
                workflows = config.workflows.len(),
                "extract_deps: parsed AgentConfig OK"
            );
            return extract_agent_deps(&config);
        }
        Err(e) => {
            tracing::warn!(error = %e, "extract_deps: parse_agent_config failed, trying raw JSON");
        }
    }
    // Fallback: parse as raw JSON for simpler frontmatter
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(frontmatter_json) {
        if let Some(plugins) = val["requires"]["plugins"].as_array() {
            for p in plugins {
                if let Some(p) = p.as_str() {
                    deps.push(DepRef {
                        dep_type: DepType::Plugin,
                        reference: p.to_string(),
                    });
                }
            }
        }
        if let Some(skills) = val["skills"].as_array() {
            for s in skills {
                if let Some(s) = s.as_str() {
                    deps.push(DepRef {
                        dep_type: DepType::Skill,
                        reference: s.to_string(),
                    });
                }
            }
        }
    }
    deps
}

/// Extract dependencies from an agent config.
/// Extracts plugin deps from `requires.plugins` and skill deps from
/// the top-level `skills` array and inline activity skill references.
pub fn extract_agent_deps(config: &napp::agent::AgentConfig) -> Vec<DepRef> {
    let mut deps = Vec::new();

    // Plugin dependencies from requires block — installed before skills
    for plugin_ref in &config.requires.plugins {
        deps.push(DepRef {
            dep_type: DepType::Plugin,
            reference: plugin_ref.clone(),
        });
    }

    for skill_ref in &config.skills {
        deps.push(DepRef {
            dep_type: DepType::Skill,
            reference: skill_ref.clone(),
        });
    }
    // Also extract skill refs from inline activities
    for binding in config.workflows.values() {
        for activity in &binding.activities {
            for skill_name in &activity.skills {
                deps.push(DepRef {
                    dep_type: DepType::Skill,
                    reference: skill_name.clone(),
                });
            }
        }
    }
    deps
}

/// Extract dependencies from a workflow definition.
pub fn extract_workflow_deps(def: &workflow::parser::WorkflowDef) -> Vec<DepRef> {
    let mut deps = Vec::new();
    let mut seen = HashSet::new();

    // From dependencies block
    for s in &def.dependencies.skills {
        if seen.insert(format!("skill:{}", s)) {
            deps.push(DepRef {
                dep_type: DepType::Skill,
                reference: s.clone(),
            });
        }
    }
    for w in &def.dependencies.workflows {
        if seen.insert(format!("workflow:{}", w)) {
            deps.push(DepRef {
                dep_type: DepType::Workflow,
                reference: w.clone(),
            });
        }
    }

    // From activities
    for activity in &def.activities {
        for skill_name in &activity.skills {
            if seen.insert(format!("skill:{}", skill_name)) {
                deps.push(DepRef {
                    dep_type: DepType::Skill,
                    reference: skill_name.clone(),
                });
            }
        }
    }

    deps
}

/// Extract dependencies from a skill (inter-skill deps + plugin deps).
pub fn extract_skill_deps(skill: &tools::skills::Skill) -> Vec<DepRef> {
    let mut deps = Vec::new();

    for dep_name in &skill.dependencies {
        deps.push(DepRef {
            dep_type: DepType::Skill,
            reference: dep_name.clone(),
        });
    }

    // Extract plugin dependencies (non-optional only)
    for plugin in &skill.plugins {
        if !plugin.optional {
            deps.push(DepRef {
                dep_type: DepType::Plugin,
                reference: plugin.name.clone(),
            });
        }
    }

    deps
}

// ── API Client Helper ───────────────────────────────────────────────

pub(crate) fn build_api_client(state: &AppState) -> Result<NeboLoopApi, types::NeboError> {
    crate::codes::build_api_client(state)
}

// ── HTTP Handler ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ApproveRequest {
    pub deps: Vec<DepRef>,
}

/// POST /deps/approve — force-install previously pending deps.
pub async fn approve_deps(
    State(state): State<AppState>,
    Json(body): Json<ApproveRequest>,
) -> HandlerResult<serde_json::Value> {
    let mut visited = HashSet::new();
    let result = resolve_cascade_force(&state, body.deps, &mut visited).await;
    Ok(Json(serde_json::json!(result)))
}
