//! Data directory migration to sealed .napp layout.
//!
//! On first startup with the new directory structure:
//! - Moves `skills/*.yaml` and `skills/*/SKILL.md` → `user/skills/`
//! - Moves `tools/` contents → `user/tools/` (sideloaded tools)
//! - Marker file `.migrated-v2` prevents re-running.

use std::path::Path;
use tracing::{debug, info, warn};

const MIGRATION_MARKER: &str = ".migrated-v2";

/// Run the data directory migration if it hasn't been run yet.
///
/// Call this during server startup after `ensure_artifact_dirs()`.
pub fn migrate_if_needed(data_dir: &Path) {
    let marker = data_dir.join(MIGRATION_MARKER);
    if marker.exists() {
        return;
    }

    info!("running one-time migration to sealed .napp directory layout");

    migrate_skills(data_dir);
    migrate_tools(data_dir);

    // Write migration marker
    if let Err(e) = std::fs::write(&marker, "migrated") {
        warn!(error = %e, "failed to write migration marker");
    }

    info!("migration complete");
}

/// Move skills from `<data_dir>/skills/` → `<data_dir>/user/skills/`
fn migrate_skills(data_dir: &Path) {
    let old_skills = data_dir.join("skills");
    let new_skills = data_dir.join("user").join("skills");

    if !old_skills.exists() {
        return;
    }

    let entries = match std::fs::read_dir(&old_skills) {
        Ok(e) => e,
        Err(e) => {
            warn!(error = %e, "failed to read old skills directory");
            return;
        }
    };

    let mut count = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();

        if path.is_dir() {
            // Directory-based skill (contains SKILL.md)
            let dest = new_skills.join(&name);
            if !dest.exists() {
                if let Err(e) = copy_dir_recursive(&path, &dest) {
                    warn!(src = %path.display(), error = %e, "failed to migrate skill directory");
                    continue;
                }
                count += 1;
            }
        } else {
            // Flat file (.yaml or .yaml.disabled)
            let name_str = name.to_string_lossy();
            if name_str.ends_with(".yaml") || name_str.ends_with(".yaml.disabled") {
                let dest = new_skills.join(&name);
                if !dest.exists() {
                    if let Err(e) = std::fs::copy(&path, &dest) {
                        warn!(src = %path.display(), error = %e, "failed to migrate skill file");
                        continue;
                    }
                    count += 1;
                }
            }
        }
    }

    if count > 0 {
        info!(count, "migrated skills to user/skills/");
    }
}

/// Move tools from `<data_dir>/tools/` → `<data_dir>/user/tools/`
fn migrate_tools(data_dir: &Path) {
    let old_tools = data_dir.join("tools");
    let new_tools = data_dir.join("user").join("tools");

    if !old_tools.exists() {
        return;
    }

    let entries = match std::fs::read_dir(&old_tools) {
        Ok(e) => e,
        Err(e) => {
            warn!(error = %e, "failed to read old tools directory");
            return;
        }
    };

    let mut count = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip hidden dirs (.tmp, etc.)
        if name_str.starts_with('.') {
            continue;
        }

        let dest = new_tools.join(&name);
        if !dest.exists() {
            if path
                .symlink_metadata()
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
            {
                // Preserve symlinks (sideloaded tools)
                if let Ok(target) = std::fs::read_link(&path) {
                    #[cfg(unix)]
                    {
                        if let Err(e) = std::os::unix::fs::symlink(&target, &dest) {
                            warn!(src = %path.display(), error = %e, "failed to migrate tool symlink");
                            continue;
                        }
                    }
                    #[cfg(windows)]
                    {
                        if let Err(e) = std::os::windows::fs::symlink_dir(&target, &dest) {
                            warn!(src = %path.display(), error = %e, "failed to migrate tool symlink");
                            continue;
                        }
                    }
                }
            } else {
                if let Err(e) = copy_dir_recursive(&path, &dest) {
                    warn!(src = %path.display(), error = %e, "failed to migrate tool directory");
                    continue;
                }
            }
            count += 1;
        }
    }

    if count > 0 {
        info!(count, "migrated tools to user/tools/");
    }
}

// ── Phase 3: Extract sealed .napp archives alongside ────────────────

const EXTRACTION_MARKER: &str = ".migrated-v3";

/// Extract all .napp archives in nebo/ to sibling directories.
///
/// Idempotent: skips archives whose sibling directory already exists.
/// Marker `.migrated-v3` prevents re-running the walk.
pub fn migrate_napp_extraction(data_dir: &Path) {
    let marker = data_dir.join(EXTRACTION_MARKER);
    if marker.exists() {
        return;
    }

    let nebo_dir = data_dir.join("nebo");
    if !nebo_dir.exists() {
        // No sealed archives to extract
        if let Err(e) = std::fs::write(&marker, "migrated") {
            warn!(error = %e, "failed to write extraction migration marker");
        }
        return;
    }

    info!("extracting .napp archives to sibling directories");

    let mut count = 0usize;
    for subdir in &["skills", "workflows", "agents"] {
        let dir = nebo_dir.join(subdir);
        if dir.exists() {
            count += extract_napps_recursive(&dir);
        }
    }

    if count > 0 {
        info!(count, "extracted .napp archives");
    }

    if let Err(e) = std::fs::write(&marker, "migrated") {
        warn!(error = %e, "failed to write extraction migration marker");
    }
}

/// Walk a directory tree and extract every .napp file alongside itself.
///
/// Skips sealed .napp files (paid content) — those are read in memory at runtime
/// and never fully extracted to disk. Detection: after unwrapping the envelope,
/// if the payload does NOT start with gzip magic bytes, it's sealed.
fn extract_napps_recursive(dir: &Path) -> usize {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };

    let mut count = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            count += extract_napps_recursive(&path);
        } else if path.extension().is_some_and(|ext| ext == "napp") {
            // Check if this .napp is sealed (encrypted) — skip if so
            if napp::reader::is_sealed_napp(&path) {
                debug!(path = %path.display(), "skipping sealed .napp (paid content)");
                continue;
            }
            match napp::reader::extract_napp_alongside(&path) {
                Ok(_) => count += 1,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "failed to extract .napp archive");
                }
            }
        }
    }
    count
}

// ── Phase 4: Rename roles/ → agents/ and ROLE.md → AGENT.md ─────────

#[allow(dead_code)] // One-time migration, kept for users upgrading from older versions
const ROLES_TO_AGENTS_MARKER: &str = ".migrated-v4";

/// Rename `roles/` directories to `agents/` and ROLE.md/role.json → AGENT.md/agent.json.
///
/// Must run BEFORE `ensure_artifact_dirs()` so the renamed directories are in place
/// before the directory structure is validated.
#[allow(dead_code)]
pub fn migrate_roles_to_agents(data_dir: &Path) {
    let marker = data_dir.join(ROLES_TO_AGENTS_MARKER);
    if marker.exists() {
        return;
    }

    info!("migrating roles/ → agents/ directory layout");

    let mut moved = 0usize;
    let mut renamed = 0usize;

    for namespace in &["nebo", "user"] {
        let roles_dir = data_dir.join(namespace).join("roles");
        let agents_dir = data_dir.join(namespace).join("agents");

        if roles_dir.exists() {
            // Move contents from roles/ into agents/
            if let Err(e) = std::fs::create_dir_all(&agents_dir) {
                warn!(error = %e, dir = %agents_dir.display(), "failed to create agents directory");
                continue;
            }

            let entries = match std::fs::read_dir(&roles_dir) {
                Ok(e) => e,
                Err(e) => {
                    warn!(error = %e, dir = %roles_dir.display(), "failed to read roles directory");
                    continue;
                }
            };

            for entry in entries.flatten() {
                let src = entry.path();
                let dest = agents_dir.join(entry.file_name());
                if !dest.exists() {
                    if let Err(e) = std::fs::rename(&src, &dest) {
                        // Cross-device? Fall back to copy + delete.
                        if let Err(e2) = copy_dir_recursive(&src, &dest) {
                            warn!(src = %src.display(), error = %e, copy_error = %e2, "failed to move agent");
                            continue;
                        }
                        let _ = std::fs::remove_dir_all(&src);
                    }
                    moved += 1;
                }
            }

            // Remove now-empty roles/ directory
            let _ = std::fs::remove_dir(&roles_dir);
        }

        // Rename ROLE.md → AGENT.md and role.json → agent.json inside agents/
        if agents_dir.exists() {
            renamed += rename_role_files_recursive(&agents_dir);
        }
    }

    if moved > 0 || renamed > 0 {
        info!(moved, renamed, "roles → agents migration complete");
    }

    if let Err(e) = std::fs::write(&marker, "migrated") {
        warn!(error = %e, "failed to write roles-to-agents migration marker");
    }
}

/// Recursively rename ROLE.md → AGENT.md, role.json → agent.json,
/// and fix manifest.json `"type": "role"` → `"type": "agent"`.
#[allow(dead_code)]
fn rename_role_files_recursive(dir: &Path) -> usize {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };

    let mut count = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            count += rename_role_files_recursive(&path);
        } else {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            // Rename role files
            let new_name = match name_str.as_ref() {
                "ROLE.md" => Some("AGENT.md"),
                "role.md" => Some("agent.md"),
                "role.json" => Some("agent.json"),
                _ => None,
            };
            if let Some(new) = new_name {
                let new_path = dir.join(new);
                if !new_path.exists() {
                    if let Err(e) = std::fs::rename(&path, &new_path) {
                        warn!(src = %path.display(), error = %e, "failed to rename role file");
                    } else {
                        count += 1;
                    }
                }
            }

            // Fix manifest.json: "type": "role" → "type": "agent", and "role" → "agent" in descriptions
            if name_str == "manifest.json" {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&content) {
                        let mut changed = false;
                        if json.get("type").and_then(|v| v.as_str()) == Some("role") {
                            json["type"] = serde_json::Value::String("agent".into());
                            changed = true;
                        }
                        if let Some(desc) = json.get("description").and_then(|v| v.as_str()) {
                            let new_desc = desc
                                .replace(" role ", " agent ")
                                .replace(" Role ", " Agent ");
                            if new_desc != desc {
                                json["description"] = serde_json::Value::String(new_desc);
                                changed = true;
                            }
                        }
                        if changed {
                            if let Ok(pretty) = serde_json::to_string_pretty(&json) {
                                if let Err(e) = std::fs::write(&path, pretty) {
                                    warn!(path = %path.display(), error = %e, "failed to update manifest.json");
                                } else {
                                    count += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    count
}

// ── Phase 5: Data directory location ─────────────────────────────────

/// No-op. Previously migrated data into `~/.nebo/`.
///
/// Nebo now uses the platform-native data directory again (see
/// `config::data_dir()`):
///   - macOS:   ~/Library/Application Support/Nebo
///   - Windows: %APPDATA%\Nebo
///   - Linux:   ~/.local/share/nebo
///
/// The old migration moved data *into* `~/.nebo/`, which now fights the
/// platform-native location. All beta installs are fresh, so no data
/// migration is needed. This is kept as a no-op so the call site compiles.
pub fn migrate_data_dir() {}

// ── Phase 5b: Orphaned agent crons → agent workflows ─────────────────

/// One-time self-heal: convert generic assistant-owned cron jobs that carry a
/// named agent's recurring duty into that agent's own workflow bindings.
///
/// Pre-v0.12.13 the agent tool schema hid `automations`, so chat-created
/// agents ended up as bare personas while their schedules leaked into
/// `task_type = "agent"` cron jobs owned by the assistant (wrong identity,
/// invisible to the Workflows tab and Schedule page). This walks every agent
/// with zero workflow bindings, claims the crons whose name matches the
/// agent's slug, converts each into a schedule binding (prompt → single-step
/// activity), and deletes the orphaned cron.
///
/// Idempotent by construction: converted crons are deleted and only agents
/// with zero bindings are considered — no marker file needed.
pub fn migrate_orphaned_agent_crons(store: &db::Store) {
    let agents = match store.list_agents(1000, 0) {
        Ok(a) => a,
        Err(_) => return,
    };
    let crons = match store.list_cron_jobs(1000, 0) {
        Ok(c) => c,
        Err(_) => return,
    };

    // "Price Watch" → "price-watch"; "content_creator_daily" → "content-creator-daily"
    let norm = |s: &str| -> String {
        let mut out = String::new();
        let mut dash = false;
        for c in s.chars() {
            if c.is_ascii_alphanumeric() {
                out.push(c.to_ascii_lowercase());
                dash = false;
            } else if !dash && !out.is_empty() {
                out.push('-');
                dash = true;
            }
        }
        out.trim_end_matches('-').to_string()
    };

    for agent in agents.iter().filter(|a| a.id != "assistant") {
        // Only bare agents — never touch agents that already own bindings.
        match store.list_agent_workflows(&agent.id) {
            Ok(w) if w.is_empty() => {}
            _ => continue,
        }
        let slug = norm(&agent.name);
        let slug_short = slug.strip_suffix("-agent").unwrap_or(&slug).to_string();
        if slug_short.len() < 4 {
            continue; // refuse to fuzzy-match very short names
        }

        let mut fm: serde_json::Value =
            serde_json::from_str(&agent.frontmatter).unwrap_or_else(|_| serde_json::json!({}));
        let mut converted = 0usize;

        for job in crons.iter().filter(|j| {
            j.task_type == "agent"
                && matches!(j.agent_id.as_deref(), None | Some("") | Some("assistant"))
        }) {
            let jn = norm(&job.name);
            if !(jn.starts_with(&slug) || jn.starts_with(&slug_short)) {
                continue;
            }
            let prompt = job.message.clone().unwrap_or_default();
            if prompt.is_empty() {
                continue;
            }

            let binding_name = jn
                .strip_prefix(slug.as_str())
                .or_else(|| jn.strip_prefix(slug_short.as_str()))
                .unwrap_or(jn.as_str())
                .trim_matches('-')
                .to_string();
            let binding_name = if binding_name.is_empty() {
                jn.clone()
            } else {
                binding_name
            };

            let cron = tools::PersonaTool::normalize_cron(&job.schedule);
            let desc = format!("Migrated from cron job '{}'", job.name);
            let activities =
                serde_json::json!([{ "id": "run", "intent": job.name, "steps": [prompt] }]);

            if store
                .upsert_agent_workflow(
                    &agent.id,
                    &binding_name,
                    "schedule",
                    &cron,
                    Some(&desc),
                    None,
                    None,
                    Some(&activities.to_string()),
                    None,
                    false,
                )
                .is_err()
            {
                continue;
            }

            fm["workflows"][&binding_name] = serde_json::json!({
                "trigger": { "type": "schedule", "cron": cron },
                "description": desc,
                "activities": activities,
            });

            let _ = store.delete_cron_job_by_name(&job.name);
            converted += 1;
        }

        if converted > 0 {
            let _ = store.update_agent(
                &agent.id,
                &agent.name,
                &agent.description,
                &agent.agent_md,
                &fm.to_string(),
                agent.pricing_model.as_deref(),
                agent.pricing_cost,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            );
            // Persist agent.json to the agent's directory too — the filesystem
            // is authoritative: the agent FS watcher re-syncs DB rows from disk
            // on scan, and a dir with only AGENT.md would clobber the DB
            // frontmatter written above with an empty one.
            crate::handlers::agents::write_agent_json_to_fs(&agent.napp_path, &fm);
            if let Ok(bindings) = store.list_agent_workflows(&agent.id) {
                workflow::triggers::register_agent_triggers(&agent.id, &bindings, store);
            }
            info!(
                agent = %agent.name,
                converted,
                "migrated orphaned crons into agent workflows"
            );
        }
    }
}

// ── Phase 5c: Sidecar data out of the shared folders ─────────────────

/// Written under `appdata/` once sidecar data has moved to per-artifact
/// folders; it holds the record of what moved.
const APP_DATA_MARKER: &str = ".app-data-per-app";

/// A sidecar whose data may sit where the old rule put it.
struct DataOwner {
    /// Who it is, for the record (an agent id, a tool's folder name).
    who: String,
    /// Its code folder (the one holding `manifest.json`).
    tool_dir: std::path::PathBuf,
    /// Its own data folder now (`napp::app_data::data_dir`).
    data_dir: std::path::PathBuf,
}

/// One-time move of sidecar data into per-artifact folders.
///
/// A sidecar's data folder used to be named after its code folder's PARENT,
/// so every app under `user/agents/<Name>/` shared `appdata/plugins/agents/`
/// and every loose tool under `user/tools/<name>/` shared
/// `appdata/plugins/tools/`. Each app now has `appdata/agents/<agent id>/`
/// and each tool `appdata/plugins/<slug>/` (`napp::app_data::data_dir`).
/// This moves what the old rule wrote into those folders, before any sidecar
/// starts. Marker: `appdata/.app-data-per-app`.
pub fn migrate_app_data_per_app(data_dir: &Path, store: &db::Store) {
    use napp::app_data::{DataKind, data_dir as own_dir};

    if data_dir.join("appdata").join(APP_DATA_MARKER).exists() {
        return;
    }
    let mut owners = Vec::new();
    match store.list_agents(1000, 0) {
        Ok(agents) => {
            for agent in agents.iter().filter(|a| a.is_app.unwrap_or(0) != 0) {
                let Some(tool_dir) = crate::handlers::agents::app_tool_dir(agent) else { continue };
                if !crate::app_lifecycle::has_sidecar(agent, &tool_dir) {
                    continue;
                }
                if let Some(dir) = own_dir(data_dir, DataKind::App, &agent.id) {
                    owners.push(DataOwner { who: agent.id.clone(), tool_dir, data_dir: dir });
                }
            }
        }
        Err(e) => {
            // Without the apps there is no knowing whose data is whose; try
            // again next start rather than record a move that never ran.
            warn!(error = %e, "could not list apps; sidecar data not moved this start");
            return;
        }
    }
    let user_tools = data_dir.join("user").join("tools");
    for entry in std::fs::read_dir(&user_tools).into_iter().flatten().flatten() {
        let tool_dir = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !tool_dir.join("manifest.json").is_file() {
            continue;
        }
        if let Some(dir) = own_dir(data_dir, DataKind::Tool, &name) {
            owners.push(DataOwner { who: name, tool_dir, data_dir: dir });
        }
    }
    move_shared_sidecar_data(data_dir, &owners);
}

/// Where the old rule put a sidecar's data: `appdata/<agents|plugins>/<name of
/// the code folder's parent>/`, `agents` only for a manifest typed `agent`.
/// `None` when the manifest cannot be read (the old launch refused those, so
/// they never wrote data).
fn legacy_sidecar_data_dir(data_dir: &Path, tool_dir: &Path) -> Option<std::path::PathBuf> {
    let manifest = napp::Manifest::load(&tool_dir.join("manifest.json")).ok()?;
    let slug = tool_dir
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or(&manifest.name)
        .to_string();
    let kind = if manifest.artifact_type == "agent" { "agents" } else { "plugins" };
    Some(data_dir.join("appdata").join(kind).join(slug))
}

/// Move each owner's data from the folder the old rule gave it into its own.
///
/// - An old folder only one owner used moves to it, entry by entry, and is
///   removed once empty.
/// - An old folder more than one owner used (two apps, or an app and the
///   plugin of the same name) cannot be split by ownership: each entry is
///   copied to every owner that used it and the original stays.
/// - Nothing is deleted or overwritten: an entry whose destination already
///   exists stays where it was.
///
/// Every step is logged and written into the marker. Returns the record.
fn move_shared_sidecar_data(data_dir: &Path, owners: &[DataOwner]) -> Vec<String> {
    let appdata = data_dir.join("appdata");
    let marker = appdata.join(APP_DATA_MARKER);
    if marker.exists() {
        return Vec::new();
    }

    // Old folder → the owners that used it.
    let mut groups: Vec<(std::path::PathBuf, Vec<&DataOwner>)> = Vec::new();
    for owner in owners {
        let Some(old) = legacy_sidecar_data_dir(data_dir, &owner.tool_dir) else { continue };
        if old == owner.data_dir {
            continue;
        }
        match groups.iter_mut().find(|(o, _)| *o == old) {
            Some((_, users)) => users.push(owner),
            None => groups.push((old, vec![owner])),
        }
    }

    let mut record = Vec::new();
    for (old, users) in &groups {
        let Ok(entries) = std::fs::read_dir(old) else { continue };
        let mut entries: Vec<std::path::PathBuf> = entries.flatten().map(|e| e.path()).collect();
        if entries.is_empty() {
            continue;
        }
        entries.sort();
        let plugin_too = old.parent() == Some(appdata.join("plugins").as_path())
            && old.file_name().is_some_and(|slug| {
                data_dir.join("nebo").join("plugins").join(slug).exists()
                    || data_dir.join("user").join("plugins").join(slug).exists()
            });
        let shared = users.len() > 1 || plugin_too;
        if shared {
            let who: Vec<&str> = users.iter().map(|u| u.who.as_str()).collect();
            let with = if plugin_too { " and the plugin of the same name" } else { "" };
            record.push(format!(
                "{} was used by {}{with}: copying each entry to each, originals kept",
                old.display(),
                who.join(", ")
            ));
        }
        for entry in &entries {
            let Some(name) = entry.file_name() else { continue };
            for user in users {
                let dest = user.data_dir.join(name);
                let step = if dest.symlink_metadata().is_ok() {
                    format!("{}: kept {} ({} already exists)", user.who, entry.display(), dest.display())
                } else if let Err(e) = std::fs::create_dir_all(&user.data_dir) {
                    format!("{}: kept {} (could not create {}: {e})", user.who, entry.display(), user.data_dir.display())
                } else if shared {
                    let copied = if entry.is_dir() {
                        copy_dir_recursive(entry, &dest)
                    } else {
                        std::fs::copy(entry, &dest).map(|_| ())
                    };
                    match copied {
                        Ok(()) => format!("{}: copied {} -> {}", user.who, entry.display(), dest.display()),
                        Err(e) => format!("{}: kept {} (copy failed: {e})", user.who, entry.display()),
                    }
                } else {
                    match std::fs::rename(entry, &dest) {
                        Ok(()) => format!("{}: moved {} -> {}", user.who, entry.display(), dest.display()),
                        Err(e) => format!("{}: kept {} (move failed: {e})", user.who, entry.display()),
                    }
                };
                record.push(step);
            }
        }
        if !shared && std::fs::remove_dir(old).is_ok() {
            record.push(format!("removed the emptied {}", old.display()));
        }
    }

    for step in &record {
        info!("sidecar data: {step}");
    }
    let text = if record.is_empty() { "nothing to move\n".to_string() } else { record.join("\n") + "\n" };
    if let Err(e) = std::fs::create_dir_all(&appdata).and_then(|()| std::fs::write(&marker, text)) {
        warn!(error = %e, "could not record the sidecar data move; it runs again next start");
    }
    record
}

// ── Phase 6: Seed bundled .napp files from app resources ──────────

/// Seed `.napp` files from app bundle resources into the data directory.
///
/// On desktop installs, the app bundle ships with pre-signed `.napp` files in
/// `bundled-napps/{skills,agents,plugins}/`. This function copies them into the
/// writable data directory so Phase 3 extraction (and plugin install) can process
/// them.
///
/// Marker: `.bundled-<app_version>` — re-runs on app upgrade so new bundled
/// content is seeded.
pub fn seed_bundled_napps(data_dir: &Path) {
    let app_version = env!("CARGO_PKG_VERSION");
    let marker = data_dir.join(format!(".bundled-{}", app_version));
    if marker.exists() {
        return;
    }

    let resources_dir = match config::bundled_napps_dir() {
        Some(d) => d,
        None => {
            // No bundled resources (dev/CLI mode) — write marker and return
            let _ = std::fs::write(&marker, "no-resources");
            return;
        }
    };

    info!(dir = %resources_dir.display(), "seeding bundled .napp files");

    let nebo_dir = data_dir.join("nebo");
    let mut count = 0usize;

    // Skills and agents: copy .napp files → nebo/{skills,agents}/
    // Phase 3 (migrate_napp_extraction) handles the actual extraction.
    for artifact_type in &["skills", "agents"] {
        let src_dir = resources_dir.join(artifact_type);
        if !src_dir.is_dir() {
            continue;
        }
        let dest_dir = nebo_dir.join(artifact_type);
        let _ = std::fs::create_dir_all(&dest_dir);
        count += seed_napp_files(&src_dir, &dest_dir);
    }

    // Plugins: verify envelope + extract directly to nebo/plugins/<slug>/<version>/
    let plugins_src = resources_dir.join("plugins");
    if plugins_src.is_dir() {
        count += seed_plugin_napps(&plugins_src, &nebo_dir.join("plugins"));
    }

    if count > 0 {
        info!(count, "seeded bundled .napp files");

        // Remove the Phase 3 extraction marker so newly seeded .napp files
        // get extracted on this startup.
        let extraction_marker = data_dir.join(EXTRACTION_MARKER);
        if extraction_marker.exists() {
            let _ = std::fs::remove_file(&extraction_marker);
            info!("cleared extraction marker — Phase 3 will re-run for new .napp files");
        }
    }

    if let Err(e) = std::fs::write(&marker, "seeded") {
        warn!(error = %e, "failed to write bundled seed marker");
    }
}

/// Copy .napp files from `src_dir` to `dest_dir`, preserving subdirectory structure.
/// Skips files that already exist at the destination.
fn seed_napp_files(src_dir: &Path, dest_dir: &Path) -> usize {
    seed_napp_files_recursive(src_dir, dest_dir, src_dir)
}

fn seed_napp_files_recursive(dir: &Path, dest_base: &Path, src_base: &Path) -> usize {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };

    let mut count = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            count += seed_napp_files_recursive(&path, dest_base, src_base);
        } else if path.extension().is_some_and(|ext| ext == "napp") {
            // Preserve relative path structure
            let rel = match path.strip_prefix(src_base) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let dest = dest_base.join(rel);
            if dest.exists() {
                continue;
            }
            if let Some(parent) = dest.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::copy(&path, &dest) {
                Ok(_) => {
                    info!(src = %path.display(), dest = %dest.display(), "seeded .napp");
                    count += 1;
                }
                Err(e) => {
                    warn!(src = %path.display(), error = %e, "failed to seed .napp file");
                }
            }
        }
    }
    count
}

/// Seed plugin .napp files: verify envelope, extract to nebo/plugins/<slug>/<version>/.
///
/// Plugins use a different directory layout than skills/agents, so we can't
/// just copy the .napp and let Phase 3 handle it. Instead we:
/// 1. Verify the .napp envelope with the embedded NeboAI public key
/// 2. Read plugin.json from the tar.gz to get slug + version
/// 3. Skip if nebo/plugins/<slug>/<version>/ already exists
/// 4. Store the .napp and extract alongside
fn seed_plugin_napps(src_dir: &Path, plugins_dest: &Path) -> usize {
    let entries = match std::fs::read_dir(src_dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };

    let mut count = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.extension().is_some_and(|ext| ext == "napp") {
            continue;
        }

        match seed_single_plugin(&path, plugins_dest) {
            Ok(true) => count += 1,
            Ok(false) => {} // skipped (already exists)
            Err(e) => {
                warn!(path = %path.display(), error = %e, "failed to seed bundled plugin");
            }
        }
    }
    count
}

/// Seed a single plugin .napp. Returns Ok(true) if installed, Ok(false) if skipped.
fn seed_single_plugin(
    napp_path: &Path,
    plugins_dest: &Path,
) -> Result<bool, Box<dyn std::error::Error>> {
    let data = std::fs::read(napp_path)?;

    // Verify envelope (magic + SHA256 + ED25519 with embedded key)
    let payload = napp::napp::unwrap_napp_builtin(&data)?;

    // Read plugin.json from tar.gz to get slug + version
    let (slug, version) = napp::reader::read_plugin_identity_from_tar_gz(&payload)?;

    // Skip if already installed
    let version_dir = plugins_dest.join(&slug).join(&version);
    if version_dir.exists() {
        return Ok(false);
    }

    // Store .napp and extract alongside
    let plugin_dir = plugins_dest.join(&slug);
    std::fs::create_dir_all(&plugin_dir)?;

    let dest_napp = plugin_dir.join(format!("{}.napp", version));
    std::fs::write(&dest_napp, &data)?;

    // extract_napp_alongside: <slug>/<version>.napp → <slug>/<version>/
    napp::reader::extract_napp_alongside(&dest_napp)?;

    // Set +x on any binary in the extracted dir
    set_executable_in_dir(&version_dir);

    info!(
        plugin = slug,
        version = version,
        path = %version_dir.display(),
        "seeded bundled plugin"
    );

    Ok(true)
}

/// Set +x on executables in a directory (plugin binaries).
#[cfg(unix)]
fn set_executable_in_dir(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        // Skip metadata files
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.ends_with(".json") || name_str.ends_with(".md") {
            continue;
        }
        // Check if it's a native binary (ELF/Mach-O/PE)
        if let Ok(data) = std::fs::read(&path) {
            if data.len() >= 4 {
                let is_native = data.starts_with(&[0x7f, 0x45, 0x4c, 0x46]) // ELF
                    || data.starts_with(&[0xfe, 0xed, 0xfa, 0xce])          // Mach-O 32
                    || data.starts_with(&[0xfe, 0xed, 0xfa, 0xcf])          // Mach-O 64
                    || data.starts_with(&[0xce, 0xfa, 0xed, 0xfe])          // Mach-O 32 (swapped)
                    || data.starts_with(&[0xcf, 0xfa, 0xed, 0xfe])          // Mach-O 64 (swapped)
                    || data.starts_with(&[0xca, 0xfe, 0xba, 0xbe])          // Universal
                    || data.starts_with(&[0x4d, 0x5a]); // PE
                if is_native {
                    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
                }
            }
        }
    }
}

#[cfg(not(unix))]
fn set_executable_in_dir(_dir: &Path) {
    // No-op on Windows — executables don't need +x
}

/// Recursively copy a directory.
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_migrate_skills() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path();

        // Create old layout
        let old_skills = data_dir.join("skills");
        std::fs::create_dir_all(&old_skills).unwrap();
        std::fs::write(old_skills.join("legacy.yaml"), "content").unwrap();
        std::fs::write(old_skills.join("disabled.yaml.disabled"), "off").unwrap();
        let skill_dir = old_skills.join("my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "---\nname: test\n---\nbody").unwrap();

        // Create new layout dirs
        let new_skills = data_dir.join("user").join("skills");
        std::fs::create_dir_all(&new_skills).unwrap();

        // Run migration
        migrate_skills(data_dir);

        // Verify
        assert!(new_skills.join("legacy.yaml").exists());
        assert!(new_skills.join("disabled.yaml.disabled").exists());
        assert!(new_skills.join("my-skill").join("SKILL.md").exists());
    }

    #[test]
    fn test_migrate_idempotent() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path();

        // Create dirs
        std::fs::create_dir_all(data_dir.join("skills")).unwrap();
        std::fs::write(data_dir.join("skills").join("test.yaml"), "v1").unwrap();
        std::fs::create_dir_all(data_dir.join("user").join("skills")).unwrap();

        // Run migration twice
        migrate_skills(data_dir);
        // Modify original
        std::fs::write(data_dir.join("skills").join("test.yaml"), "v2").unwrap();
        migrate_skills(data_dir);

        // Should keep v1 (doesn't overwrite existing)
        let content =
            std::fs::read_to_string(data_dir.join("user").join("skills").join("test.yaml"))
                .unwrap();
        assert_eq!(content, "v1");
    }

    #[test]
    fn test_marker_prevents_rerun() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path();
        std::fs::create_dir_all(data_dir.join("user").join("skills")).unwrap();

        // First run
        migrate_if_needed(data_dir);
        assert!(data_dir.join(MIGRATION_MARKER).exists());

        // Create a skill that should NOT be migrated on second run
        std::fs::create_dir_all(data_dir.join("skills")).unwrap();
        std::fs::write(data_dir.join("skills").join("new.yaml"), "content").unwrap();

        // Second run — should be skipped
        migrate_if_needed(data_dir);
        assert!(
            !data_dir
                .join("user")
                .join("skills")
                .join("new.yaml")
                .exists()
        );
    }

    #[test]
    fn test_seed_napp_files_copies_with_structure() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dest = tmp.path().join("dest");
        std::fs::create_dir_all(src.join("@acme")).unwrap();
        std::fs::create_dir_all(&dest).unwrap();

        // Create test .napp files
        std::fs::write(src.join("skill-a.napp"), b"fake-napp-a").unwrap();
        std::fs::write(src.join("@acme").join("skill-b.napp"), b"fake-napp-b").unwrap();

        let count = seed_napp_files(&src, &dest);
        assert_eq!(count, 2);
        assert!(dest.join("skill-a.napp").exists());
        assert!(dest.join("@acme").join("skill-b.napp").exists());
    }

    #[test]
    fn test_seed_napp_files_skips_existing() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dest = tmp.path().join("dest");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dest).unwrap();

        // Pre-existing file at destination
        std::fs::write(dest.join("existing.napp"), b"original").unwrap();
        std::fs::write(src.join("existing.napp"), b"newer").unwrap();
        std::fs::write(src.join("new.napp"), b"new-content").unwrap();

        let count = seed_napp_files(&src, &dest);
        assert_eq!(count, 1); // Only new.napp

        // existing.napp should NOT be overwritten
        let content = std::fs::read_to_string(dest.join("existing.napp")).unwrap();
        assert_eq!(content, "original");
    }

    #[test]
    fn test_seed_bundled_marker_per_version() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path();
        std::fs::create_dir_all(data_dir.join("nebo").join("skills")).unwrap();

        // seed_bundled_napps writes marker based on CARGO_PKG_VERSION
        seed_bundled_napps(data_dir);

        let marker = data_dir.join(format!(".bundled-{}", env!("CARGO_PKG_VERSION")));
        assert!(marker.exists());

        // Second call is a no-op (marker exists)
        seed_bundled_napps(data_dir);
    }

    // ── Sidecar data out of the shared folders ──

    /// An app's code folder at `<data_dir>/<rel>` with a manifest, and the
    /// folder its data belongs in now.
    fn sidecar_app(data_dir: &Path, rel: &str, id: &str) -> DataOwner {
        let tool_dir = data_dir.join(rel);
        std::fs::create_dir_all(&tool_dir).unwrap();
        std::fs::write(
            tool_dir.join("manifest.json"),
            serde_json::json!({ "id": id, "name": id, "version": "1.0.0", "artifact_type": "app" }).to_string(),
        )
        .unwrap();
        let own = napp::app_data::data_dir(data_dir, napp::app_data::DataKind::App, id).unwrap();
        DataOwner { who: id.to_string(), tool_dir, data_dir: own }
    }

    fn put(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn text(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// Two apps under `user/agents/` shared `appdata/plugins/agents/`: each
    /// gets its own copy of everything, nothing is lost, and a second run is a
    /// no-op.
    #[test]
    fn two_apps_sharing_the_old_folder_each_get_their_own() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        let apps = [sidecar_app(home, "user/agents/Mail", "mail-app"), sidecar_app(home, "user/agents/Board", "board-app")];
        let shared = home.join("appdata/plugins/agents");
        put(&shared.join("service_url"), "http://localhost:18083");
        put(&shared.join("sidecar.log"), "both apps' lines\n");
        put(&shared.join("db/app.db"), "rows");

        let record = move_shared_sidecar_data(home, &apps);
        assert!(record[0].contains("mail-app, board-app"), "{record:?}");

        for id in ["mail-app", "board-app"] {
            let own = home.join("appdata/agents").join(id);
            assert_eq!(text(&own.join("service_url")), "http://localhost:18083");
            assert_eq!(text(&own.join("sidecar.log")), "both apps' lines\n");
            assert_eq!(text(&own.join("db/app.db")), "rows");
        }
        // Whose is whose is unknowable, so the originals stay.
        assert_eq!(text(&shared.join("service_url")), "http://localhost:18083");
        assert_eq!(text(&shared.join("db/app.db")), "rows");
        assert!(text(&home.join("appdata").join(APP_DATA_MARKER)).contains("copied"));

        put(&shared.join("late"), "x");
        assert!(move_shared_sidecar_data(home, &apps).is_empty(), "the second run does nothing");
        assert!(!home.join("appdata/agents/mail-app/late").exists());
    }

    /// The owner's case: one app used the old folder, so its data moves there
    /// and the emptied folder goes; a file already in the new folder is never
    /// overwritten, and the old one is kept beside it.
    #[test]
    fn a_folder_one_app_used_moves_to_it() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        let apps = [sidecar_app(home, "user/agents/neighbor-mail", "neighbor-mail")];
        let old = home.join("appdata/plugins/agents");
        put(&old.join("service_url"), "http://localhost:18083");
        put(&old.join("sidecar.log"), "old log\n");
        let own = home.join("appdata/agents/neighbor-mail");
        put(&own.join("sidecar.log"), "new log\n");

        move_shared_sidecar_data(home, &apps);

        assert_eq!(text(&own.join("service_url")), "http://localhost:18083");
        assert!(!old.join("service_url").exists(), "moved, not copied");
        assert_eq!(text(&own.join("sidecar.log")), "new log\n", "never overwritten");
        assert_eq!(text(&old.join("sidecar.log")), "old log\n", "kept where it was");

        // Once everything in it has moved, the old folder goes.
        std::fs::remove_file(old.join("sidecar.log")).unwrap();
        put(&old.join("notes.json"), "{}");
        std::fs::remove_file(home.join("appdata").join(APP_DATA_MARKER)).unwrap();
        move_shared_sidecar_data(home, &apps);
        assert_eq!(text(&own.join("notes.json")), "{}");
        assert!(!old.exists(), "an emptied old folder is removed");
    }

    /// A marketplace app's old folder `appdata/plugins/<slug>/` that the
    /// plugin of the same slug also uses is copied, never taken from the plugin.
    #[test]
    fn an_old_folder_a_plugin_also_uses_is_copied() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        let id = "7f1c0e2a-0000-4000-8000-000000000001";
        let apps = [sidecar_app(home, "nebo/agents/gmail/1.0.0", id)];
        std::fs::create_dir_all(home.join("nebo/plugins/gmail/2.0.0")).unwrap();
        let old = home.join("appdata/plugins/gmail");
        put(&old.join("token.json"), "secret");

        move_shared_sidecar_data(home, &apps);

        assert_eq!(text(&home.join("appdata/agents").join(id).join("token.json")), "secret");
        assert_eq!(text(&old.join("token.json")), "secret", "the plugin keeps its file");
    }
}
