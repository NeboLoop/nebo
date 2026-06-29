//! Dependency install cascade resolver.
//!
//! Walks the STRAP hierarchy (AGENT → WORK → SKILL) downward, checks local
//! presence, and installs whatever is missing. There is ONE install path:
//! every explicit install (a pasted code, a marketplace install, a collection)
//! force-installs its declared dependencies — choosing to install an artifact
//! IS consent to install the components it requires.
//!
//! The `auto_install_deps` setting gates ONLY the implicit boot-time reconcile
//! cascades in `lib.rs` (filesystem agents discovered on startup), so we don't
//! auto-pull a pile of deps for every agent on every launch without consent.

use std::collections::HashSet;

use axum::extract::State;
use axum::response::Json;
use serde::{Deserialize, Serialize};

use comm::api::NeboAIApi;

use crate::handlers::HandlerResult;
use crate::state::AppState;

// ── Types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DepType {
    Skill,
    Workflow,
    Plugin,
    Agent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepRef {
    #[serde(rename = "depType")]
    pub dep_type: DepType,
    pub reference: String,
    /// Human-readable display name, when the source knows it (e.g. collection
    /// items carry a `name`). The UI shows this instead of the opaque install
    /// code; falls back to `reference` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Canonical plugin slug, when known (collection items carry it). Plugins are
    /// stored by slug, but collections reference them by install code — which can't
    /// resolve to a slug — so this lets the cascade detect an already-installed
    /// plugin instead of re-installing it. (The artifact id can't be used: a
    /// plugin.json's `id` is publisher-supplied, not the marketplace artifact id.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
}

impl DepRef {
    /// A dependency with no known display name (the common case for refs
    /// extracted from manifests/frontmatter, which only carry codes).
    pub fn new(dep_type: DepType, reference: impl Into<String>) -> Self {
        Self {
            dep_type,
            reference: reference.into(),
            name: None,
            slug: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DepStatus {
    AlreadyInstalled,
    Installed,
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

/// Install every dependency in `deps` and recurse into their transitive deps.
///
/// This is THE cascade. It always installs — there is no "pending/approve"
/// branch any more. Explicit installs (codes, marketplace, collections) all
/// flow through here; the boot-time reconcile in `lib.rs` gates the CALL on
/// `auto_install_deps_enabled`, not the behavior inside.
pub async fn resolve_cascade(
    state: &AppState,
    deps: Vec<DepRef>,
    visited: &mut HashSet<String>,
) -> CascadeResult {
    announce_cascade_start(state, &deps);
    resolve_cascade_inner(state, deps, visited).await
}

/// Tell the UI how many top-level dependencies are about to be processed, so the
/// install modal can render a determinate progress bar instead of a spinner.
fn announce_cascade_start(state: &AppState, deps: &[DepRef]) {
    state.hub.broadcast(
        "dep_cascade_start",
        serde_json::json!({ "total": deps.len() }),
    );
}

fn resolve_cascade_inner<'a>(
    state: &'a AppState,
    deps: Vec<DepRef>,
    visited: &'a mut HashSet<String>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = CascadeResult> + Send + 'a>> {
    Box::pin(async move {
        let mut results = Vec::new();
        let mut installed_count = 0usize;
        // Retained for the CascadeResult shape the frontend reads; never
        // incremented now that the cascade always installs.
        let pending_count = 0usize;
        let mut failed_count = 0usize;

        for dep in deps {
            // Cycle / dedup check
            let key = format!("{:?}:{}", dep.dep_type, dep.reference);
            if !visited.insert(key) {
                continue;
            }

            // Check if already installed
            if is_installed(state, &dep).await {
                // Tell the UI it's present so a row shows installed (e.g. when the
                // user retries a previously-failed item that's actually installed).
                state.hub.broadcast(
                    "dep_installed",
                    serde_json::json!({
                        "depType": format!("{:?}", dep.dep_type),
                        "reference": dep.reference,
                        "name": dep.name,
                        "slug": dep.slug,
                    }),
                );
                installed_count += 1;
                results.push(DepResult {
                    dep,
                    status: DepStatus::AlreadyInstalled,
                    children: vec![],
                });
                continue;
            }

            // Bare names (no @ prefix, no install code) are built-in tool bindings —
            // mark unresolvable. EXCEPT plugins: manifests reference plugins by bare
            // slug (e.g. `gws`, `google-analytics`), which install_plugin resolves to
            // an install code via the marketplace. Skipping those left every declared
            // plugin uninstalled (the "0/N, nothing installs" bug).
            if !is_marketplace_ref(&dep.reference) && !matches!(dep.dep_type, DepType::Plugin) {
                let reason = format!(
                    "'{}' is a built-in or simple name, not a marketplace ref",
                    dep.reference
                );
                results.push(DepResult {
                    dep,
                    status: DepStatus::Unresolvable { reason },
                    children: vec![],
                });
                continue;
            }

            // Signal the UI that this dependency is now being installed so it can
            // render a live per-dependency progress indicator.
            state.hub.broadcast(
                "dep_started",
                serde_json::json!({
                    "depType": format!("{:?}", dep.dep_type),
                    "reference": dep.reference,
                    "name": dep.name,
                    "slug": dep.slug,
                }),
            );
            match install_dep(state, &dep).await {
                Ok(child_deps) => {
                    state.hub.broadcast(
                        "dep_installed",
                        serde_json::json!({
                            "depType": format!("{:?}", dep.dep_type),
                            "reference": dep.reference,
                            "name": dep.name,
                            "slug": dep.slug,
                        }),
                    );
                    installed_count += 1;

                    // Recurse into child deps
                    let child_result = resolve_cascade_inner(state, child_deps, visited).await;
                    installed_count += child_result.installed_count;
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
                            "name": dep.name,
                            "slug": dep.slug,
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

/// Whether the implicit boot-time reconcile cascade may auto-install missing
/// deps. Explicit installs ignore this — they always install. Default OFF.
pub(crate) fn auto_install_deps_enabled(state: &AppState) -> bool {
    state
        .store
        .get_settings()
        .ok()
        .flatten()
        .map(|s| s.auto_install_deps == 1)
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
        DepType::Plugin => is_plugin_installed(state, dep),
        DepType::Agent => is_agent_installed(state, &dep.reference),
    }
}

/// Plugins are stored by slug. Manifest deps reference them by slug (direct
/// resolve), but collection items reference them by install code, which doesn't
/// resolve to a slug — so fall back to the slug the collection item carries.
/// Without this, a code-referenced plugin always looks "not installed", and the
/// cascade re-installs it, surfacing a spurious failure for one already present.
fn is_plugin_installed(state: &AppState, dep: &DepRef) -> bool {
    if state.plugin_store.resolve(&dep.reference, "*").is_some() {
        return true;
    }
    // A qualified marketplace ref (`@neboloop/plugins/gws`) doesn't resolve as a
    // slug — fall back to its simple name (last segment = the plugin slug), so a
    // dep listed by qualified name is recognised as the same installed plugin
    // instead of looking absent and failing to install.
    let simple = extract_simple_name(&dep.reference);
    if simple != dep.reference && state.plugin_store.resolve(simple, "*").is_some() {
        return true;
    }
    dep.slug
        .as_deref()
        .is_some_and(|slug| state.plugin_store.resolve(slug, "*").is_some())
}

fn is_agent_installed(state: &AppState, reference: &str) -> bool {
    // Direct id match (UUID), then fall back to simple-name match against installed agents.
    if state.store.get_agent(reference).ok().flatten().is_some() {
        return true;
    }
    let simple = extract_simple_name(reference).to_lowercase();
    state
        .store
        .list_agents(1000, 0)
        .map(|agents| agents.iter().any(|a| a.name.to_lowercase() == simple))
        .unwrap_or(false)
}

fn is_skill_installed(reference: &str) -> bool {
    let simple_name = extract_simple_name(reference);
    let (Ok(user_dir), Ok(nebo_dir)) = (config::user_dir(), config::nebo_dir()) else {
        return false;
    };
    skill_present_in_root(&user_dir.join("skills"), simple_name, reference)
        || skill_present_in_root(&nebo_dir.join("skills"), simple_name, reference)
}

/// True only if THIS skill is installed under `skills_root`.
///
/// Detection is scoped to the skill's own directory (`<root>/<slug>`). An
/// unscoped "does any skill exist here" check is wrong: once the bundled skills
/// are present (they always are), every skill dependency looks installed and the
/// cascade silently skips it. Skills are stored at `<root>/<slug>/` as a loose
/// SKILL.md, an extracted version subdir (`<slug>/<version>/SKILL.md`), or a
/// sealed `<version>.napp`. `has_extracted_skill`/`has_napp_files` return false
/// for a non-existent dir, so no separate existence guard is needed.
fn skill_present_in_root(skills_root: &std::path::Path, simple_name: &str, reference: &str) -> bool {
    let dir = skills_root.join(simple_name);
    if dir.join("SKILL.md").exists() || has_extracted_skill(&dir) || has_napp_files(&dir) {
        return true;
    }

    // Install-code refs (SKIL-XXXX) are stored under the server-assigned slug,
    // not the code, so the path check above can't match — fall back to matching
    // the `code` recorded in each installed skill's manifest.json.
    if reference.starts_with("SKIL-") {
        if let Ok(entries) = std::fs::read_dir(skills_root) {
            for entry in entries.flatten() {
                let manifest = entry.path().join("manifest.json");
                if let Ok(text) = std::fs::read_to_string(&manifest) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                        if v.get("code").and_then(|c| c.as_str()) == Some(reference) {
                            return true;
                        }
                    }
                }
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
        DepType::Agent => install_agent(state, &api, &dep.reference).await,
    }
}

/// Resolve a dependency reference to a redeemable install code.
///
/// Marketplace agents declare deps as qualified names (`@org/plugins/gws`) or
/// ids, but the redeem endpoint (`api.install_skill`) only accepts install codes
/// (`PLUG-/SKIL-/AGNT-XXXX`). This bridges the two: a value that already IS a code
/// passes through; a qualified name / id is looked up in the marketplace product
/// list (match by slug = last segment, then qualifiedName) to get its `code`.
/// Without this, every qualified-name dep 404s "invalid code" and never installs.
async fn resolve_marketplace_code(
    api: &NeboAIApi,
    artifact_type: &str,
    reference: &str,
) -> Result<String, String> {
    if crate::codes::detect_code(reference).is_some() {
        return Ok(reference.to_string()); // already an install code
    }
    let slug = extract_simple_name(reference);
    // Page through the type's products and match by slug (then qualifiedName) locally.
    // Two traps avoided:
    //  - Don't pass `slug` as the query `q`: it's a fuzzy NAME search, so a hyphenated
    //    multi-word slug ("google-search-console") matches nothing.
    //  - Don't fetch one default page: the marketplace caps pageSize at 20, so deps
    //    beyond the first 20 (sentry, google-analytics) were never found. Paginate by
    //    `page` (which the backend honors only alongside pageSize) until matched/empty.
    const PAGE_SIZE: i64 = 20;
    const MAX_PAGES: i64 = 100; // safety bound (2000 items) against a non-paging backend
    let mut page = 1;
    while page <= MAX_PAGES {
        let products = api
            .list_products(Some(artifact_type), None, None, Some(page), Some(PAGE_SIZE))
            .await
            .map_err(|e| format!("resolve code for '{reference}': {e}"))?;
        let arr = match products
            .get("products")
            .or_else(|| products.get("skills"))
            .and_then(|v| v.as_array())
        {
            Some(a) if !a.is_empty() => a.clone(),
            _ => break, // no more results
        };
        let matched = arr
            .iter()
            .find(|i| i.get("slug").and_then(|s| s.as_str()) == Some(slug))
            .or_else(|| {
                arr.iter()
                    .find(|i| i.get("qualifiedName").and_then(|s| s.as_str()) == Some(reference))
            });
        if let Some(code) = matched
            .and_then(|i| i.get("code"))
            .and_then(|c| c.as_str())
            .filter(|c| !c.is_empty())
        {
            return Ok(code.to_string());
        }
        if (arr.len() as i64) < PAGE_SIZE {
            break; // last (partial) page
        }
        page += 1;
    }
    Err(format!(
        "no marketplace install code found for '{reference}' (type {artifact_type})"
    ))
}

/// Install an agent dependency via the same redeem→persist pathway used for the
/// top-level agent install (no parallel installer). Returns the new agent's own
/// dependencies so the cascade can recurse.
async fn install_agent(
    state: &AppState,
    api: &NeboAIApi,
    reference: &str,
) -> Result<Vec<DepRef>, String> {
    let code = resolve_marketplace_code(api, "agent", reference).await?;
    let resp = api
        .install_skill(&code)
        .await
        .map_err(|e| format!("install_agent: {}", e))?;

    let artifact_id = resp.artifact.id.clone();
    let name = resp.artifact.name.clone();

    if let Err(e) =
        tools::persist_agent_from_api(api, &artifact_id, &name, reference, &state.store).await
    {
        return Err(format!("persist agent {}: {}", name, e));
    }
    tracing::info!(reference, name = %name, "cascade: persisted agent");

    // Shared post-persist finalization (loader reload, update-tracking seed, roster
    // broadcast, workflow materialization) — the SAME routine the single-agent
    // install (codes.rs handle_agent_code) uses, so the cascade can't drift from it.
    // The agent installs Paused; its triggers register when it's activated.
    crate::codes::finalize_agent_install(state, &artifact_id, &name).await;

    // Recurse into the agent's own dependencies.
    if let Ok(Some(agent)) = state.store.get_agent(&artifact_id) {
        if !agent.frontmatter.is_empty() {
            return Ok(extract_agent_deps_from_frontmatter(&agent.frontmatter));
        }
    }
    Ok(vec![])
}

async fn install_skill(
    state: &AppState,
    api: &NeboAIApi,
    reference: &str,
) -> Result<Vec<DepRef>, String> {
    // Resolve qualified name → code, then redeem with NeboAI to register the install
    let code = resolve_marketplace_code(api, "skill", reference).await?;
    let resp = api
        .install_skill(&code)
        .await
        .map_err(|e| format!("install_skill: {}", e))?;

    let artifact_id = resp.artifact.id.clone();
    let name = resp.artifact.name.clone();

    // Persist to disk (download .napp or write loose files)
    let skill_dir = match tools::persist_skill_from_api(
        api,
        &artifact_id,
        &name,
        reference,
        Some(&state.store),
    )
    .await
    {
        Ok(dir) => {
            tracing::info!(reference, name = %name, dir = %dir.display(), "cascade: persisted skill");
            Some(dir)
        }
        Err(e) => {
            tracing::warn!(reference, error = %e, "cascade: failed to persist skill");
            None
        }
    };

    // Seed artifact update tracking for skills
    if let Some(ref dir) = skill_dir {
        let version = dir
            .join("manifest.json")
            .to_str()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v["version"].as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "1.0.0".to_string());
        let _ = state.store.upsert_artifact_update_pref(&artifact_id, "skill", &version);
    }

    // Reload skill loader so it appears immediately. Cold reload — load_all()'s
    // warm path trusts the stale manifest and would miss the just-installed skill
    // (see SkillLoader::reload_from_disk).
    state.skill_loader.reload_from_disk().await;

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
    api: &NeboAIApi,
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
    api: &NeboAIApi,
    reference: &str,
) -> Result<Vec<DepRef>, String> {
    // Resolve qualified name → code (plugins use the same redeem endpoint)
    let code = resolve_marketplace_code(api, "plugin", reference).await?;
    let resp = api
        .install_skill(&code)
        .await
        .map_err(|e| format!("redeem plugin code: {e}"))?;

    let name = resp.artifact.name.clone();
    // Resolve strictly by the canonical slug (server guarantees it NOT NULL + unique).
    // Never derive it from the display name — fail loudly on a missing slug.
    if resp.artifact.slug.is_empty() {
        return Err(format!(
            "plugin dependency '{name}' has no slug in the redeem response; refusing to guess from the display name"
        ));
    }
    let slug = resp.artifact.slug.clone();

    // Install via the ONE shared plugin installer (resolves the binary via get_plugin,
    // downloads, installs, registers in the DB + tool/hooks). Same code path as the
    // standalone install, so the two can't drift on binary resolution again.
    crate::codes::fetch_and_install_plugin(state, api, &slug, &name)
        .await
        .map_err(|e| e.to_string())?;

    // Extract child plugin dependencies from manifest
    let child_deps = state
        .plugin_store
        .get_dependencies(&slug)
        .into_iter()
        .filter(|d| !d.optional)
        .map(|d| DepRef::new(DepType::Plugin, d.name))
        .collect();
    Ok(child_deps)
}

// ── Dep Extraction ──────────────────────────────────────────────────

/// Extract dependencies from an agent's frontmatter JSON string.
///
/// Workflows are now inline (no external refs). Only skill dependencies are extracted
/// from both the top-level `skills` array and from inline activity skill references.
pub fn extract_agent_deps_from_frontmatter(frontmatter_json: &str) -> Vec<DepRef> {
    let mut deps: Vec<DepRef> = Vec::new();
    let mut push = |dep_type: DepType, reference: String, name: Option<String>, slug: Option<String>| {
        if reference.is_empty() {
            return;
        }
        // Canonical slug = the caller's, else the simple name (last segment) for any
        // non-code ref. This is the identity the cascade emits and the UI dedups on,
        // so `gws` and `@neboloop/plugins/gws` collapse to one row.
        let slug = slug.or_else(|| {
            if crate::codes::detect_code(&reference).is_some() {
                None
            } else {
                Some(extract_simple_name(&reference).to_string())
            }
        });
        // Dedup by (type, simple-name): the same artifact is often listed two ways —
        // a bare slug in `requires.plugins` AND a qualified `@org/plugins/name` in
        // `dependencies.plugins`. Collapse to one entry, BACKFILLING the richer
        // display name / slug from whichever shape carries it (the bare-slug entry
        // has no display name; the qualified one does).
        let key = format!("{:?}:{}", dep_type, extract_simple_name(&reference));
        if let Some(existing) = deps
            .iter_mut()
            .find(|d| format!("{:?}:{}", d.dep_type, extract_simple_name(&d.reference)) == key)
        {
            if existing.name.is_none() {
                existing.name = name;
            }
            if existing.slug.is_none() {
                existing.slug = slug;
            }
            return;
        }
        deps.push(DepRef {
            dep_type,
            reference,
            name,
            slug,
        });
    };

    // 1. Runtime AgentConfig shape: requires.plugins, top-level skills, inline activities.
    if let Ok(config) = napp::agent::parse_agent_config(frontmatter_json) {
        for d in extract_agent_deps(&config) {
            push(d.dep_type, d.reference, d.name, d.slug);
        }
    }

    // 2. Marketplace-published shape: `dependencies: { agents, skills, plugins }`,
    //    plus legacy raw `requires.plugins` / top-level `skills` string arrays.
    //    Entries may be plain strings or objects { qualifiedName, id, name }.
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(frontmatter_json) {
        let dependencies = &val["dependencies"];
        for (key, dep_type) in [
            ("agents", DepType::Agent),
            ("skills", DepType::Skill),
            ("plugins", DepType::Plugin),
            ("workflows", DepType::Workflow),
        ] {
            if let Some(arr) = dependencies[key].as_array() {
                for item in arr {
                    if let Some((reference, name)) = dep_ref_from_value(item) {
                        push(dep_type.clone(), reference, name, None);
                    }
                }
            }
        }
        if let Some(plugins) = val["requires"]["plugins"].as_array() {
            for p in plugins {
                if let Some(p) = p.as_str() {
                    push(DepType::Plugin, p.to_string(), None, None);
                }
            }
        }
        if let Some(skills) = val["skills"].as_array() {
            for s in skills {
                // Only marketplace-referenced skills are installs; bare tool-binding
                // names are provided by the agent's plugins (see extract_agent_deps).
                if let Some(s) = s.as_str().filter(|s| is_marketplace_ref(s)) {
                    push(DepType::Skill, s.to_string(), None, None);
                }
            }
        }
    }

    tracing::info!(count = deps.len(), "extract_deps: extracted (merged shapes)");
    deps
}

/// A marketplace dependency entry is either a plain reference string or an object
/// `{ qualifiedName, id, name }`. Returns (reference, display name). Prefer the
/// qualified name (a marketplace ref), then the id; carry the display `name` so
/// the cascade and UI can show "Google Workspace" instead of the bare slug.
fn dep_ref_from_value(item: &serde_json::Value) -> Option<(String, Option<String>)> {
    if let Some(s) = item.as_str() {
        return Some((s.to_string(), None));
    }
    let reference = item["qualifiedName"]
        .as_str()
        .or_else(|| item["id"].as_str())?
        .to_string();
    Some((reference, item["name"].as_str().map(String::from)))
}

/// Extract dependencies from an agent config.
/// Plugin deps come from `requires.plugins`. Skill deps come from the top-level
/// `skills` array and inline activities — but ONLY marketplace-referenced skills
/// (`@org/skills/x`, `SKIL-…`). Bare names (e.g. `gws-docs-write`) are tool
/// bindings the agent's plugins provide, not separate installs — counting them
/// inflated the dependency total (Chief of Staff showed 9 instead of 1).
pub fn extract_agent_deps(config: &napp::agent::AgentConfig) -> Vec<DepRef> {
    let mut deps = Vec::new();

    // Plugin dependencies from requires block — installed before skills
    for plugin_ref in &config.requires.plugins {
        deps.push(DepRef::new(DepType::Plugin, plugin_ref.clone()));
    }

    for skill_ref in &config.skills {
        if is_marketplace_ref(skill_ref) {
            deps.push(DepRef::new(DepType::Skill, skill_ref.clone()));
        }
    }
    // Also extract marketplace skill refs from inline activities
    for binding in config.workflows.values() {
        for activity in &binding.activities {
            for skill_name in &activity.skills {
                if is_marketplace_ref(skill_name) {
                    deps.push(DepRef::new(DepType::Skill, skill_name.clone()));
                }
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
            deps.push(DepRef::new(DepType::Skill, s.clone()));
        }
    }
    for w in &def.dependencies.workflows {
        if seen.insert(format!("workflow:{}", w)) {
            deps.push(DepRef::new(DepType::Workflow, w.clone()));
        }
    }

    // From activities
    for activity in &def.activities {
        for skill_name in &activity.skills {
            if seen.insert(format!("skill:{}", skill_name)) {
                deps.push(DepRef::new(DepType::Skill, skill_name.clone()));
            }
        }
    }

    deps
}

/// Extract dependencies from a skill (inter-skill deps + plugin deps).
pub fn extract_skill_deps(skill: &tools::skills::Skill) -> Vec<DepRef> {
    let mut deps = Vec::new();

    for dep_name in &skill.dependencies {
        deps.push(DepRef::new(DepType::Skill, dep_name.clone()));
    }

    // Extract plugin dependencies (non-optional only)
    for plugin in &skill.plugins {
        if !plugin.optional {
            deps.push(DepRef::new(DepType::Plugin, plugin.name.clone()));
        }
    }

    deps
}

// ── API Client Helper ───────────────────────────────────────────────

pub(crate) fn build_api_client(state: &AppState) -> Result<NeboAIApi, types::NeboError> {
    crate::codes::build_api_client(state)
}

// ── HTTP Handler ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ApproveRequest {
    pub deps: Vec<DepRef>,
}

/// POST /deps/approve — (re)install a specific set of deps. The sole retry
/// path: the install modal calls this to retry a single dep row that failed.
pub async fn approve_deps(
    State(state): State<AppState>,
    Json(body): Json<ApproveRequest>,
) -> HandlerResult<serde_json::Value> {
    let mut visited = HashSet::new();
    let result = resolve_cascade(&state, body.deps, &mut visited).await;
    Ok(Json(serde_json::json!(result)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: skill-presence detection must be scoped to the specific skill.
    /// The old unscoped check returned `true` for ANY skill ref whenever the
    /// skills dir held even one skill (e.g. the bundled set), so the cascade
    /// marked every skill dependency `AlreadyInstalled` and never installed it.
    #[test]
    fn skill_presence_is_scoped_to_the_skill() {
        let tmp = std::env::temp_dir().join(format!("nebo-skill-scope-{}", std::process::id()));
        let root = tmp.join("skills");
        let alpha = root.join("alpha");
        std::fs::create_dir_all(&alpha).unwrap();
        std::fs::write(alpha.join("SKILL.md"), "---\nname: alpha\n---\n").unwrap();

        // The installed skill IS detected.
        assert!(skill_present_in_root(&root, "alpha", "@org/skills/alpha"));
        // A different, not-installed skill must NOT be — even though `alpha` exists.
        assert!(!skill_present_in_root(&root, "beta", "@org/skills/beta"));

        std::fs::remove_dir_all(&tmp).ok();
    }

    /// Install-code refs are stored under the server slug, not the code, so
    /// presence is matched via the `code` recorded in manifest.json.
    #[test]
    fn skill_code_matches_via_manifest() {
        let tmp = std::env::temp_dir().join(format!("nebo-skill-code-{}", std::process::id()));
        let root = tmp.join("skills");
        let dir = root.join("real-slug");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.json"), r#"{"code":"SKIL-AAAA-BBBB"}"#).unwrap();

        assert!(skill_present_in_root(&root, "SKIL-AAAA-BBBB", "SKIL-AAAA-BBBB"));
        assert!(!skill_present_in_root(&root, "SKIL-ZZZZ-ZZZZ", "SKIL-ZZZZ-ZZZZ"));

        std::fs::remove_dir_all(&tmp).ok();
    }

    /// The cascade always installs now, but built-in / simple-name refs must
    /// still be classified `Unresolvable` rather than sent to the marketplace.
    /// `is_marketplace_ref` is the gate that distinguishes the two.
    #[test]
    fn marketplace_refs_vs_builtins() {
        // Qualified names and install codes are marketplace refs.
        assert!(is_marketplace_ref("@org/skills/intake-parser"));
        assert!(is_marketplace_ref("SKIL-AAAA-BBBB"));
        assert!(is_marketplace_ref("AGNT-AAAA-BBBB"));
        assert!(is_marketplace_ref("PLUG-AAAA-BBBB"));
        assert!(is_marketplace_ref("WORK-AAAA-BBBB"));
        // Bare/simple names are built-ins — never resolved against the marketplace.
        assert!(!is_marketplace_ref("web"));
        assert!(!is_marketplace_ref("system"));
    }
}
