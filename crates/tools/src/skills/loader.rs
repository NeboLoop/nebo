use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::manifest::{self, SkillManifest};
use super::skill::{Skill, SkillSource, SkillSummary, parse_skill_frontmatter, split_frontmatter};


/// Manages loading, caching, and hot-reloading of skills from embedded
/// (bundled), sealed .napp archives (nebo/skills/), loose files (user/skills/),
/// the skills an employee package ships with (nebo|user/agents/<slug>/skills/)
/// and the learned tree (learned/skills/<agent_id>/).
pub struct Loader {
    /// User skills directory (e.g. <data_dir>/user/skills/).
    user_dir: PathBuf,
    /// Installed (marketplace) skills directory (e.g. <data_dir>/nebo/skills/).
    installed_dir: PathBuf,
    /// Learned skills root (e.g. <data_dir>/learned/skills/), laid out as
    /// <root>/<agent_id>/<skill>/SKILL.md. Written by the self-improvement
    /// loop; skills here are per-employee (see `Skill::visible_to`).
    learned_dir: Option<PathBuf>,
    /// Employee-package roots (e.g. <data_dir>/nebo/agents/, <data_dir>/user/agents/).
    /// A package ships its own procedures at
    /// `<root>/<slug>[/<version>]/skills/<name>/SKILL.md`; they belong to the
    /// seat that shipped them and are keyed per employee, exactly like a
    /// learned skill (see `Skill::visible_to`).
    agent_dirs: Vec<PathBuf>,
    /// Loaded skills keyed by name.
    skills: Arc<RwLock<HashMap<String, Skill>>>,
    /// Optional plugin store for verifying plugin dependencies.
    plugin_store: Option<Arc<napp::plugin::PluginStore>>,
    /// Optional DB store for checking plugin enabled/disabled state.
    db_store: Option<Arc<db::Store>>,
    /// When true, the filesystem watcher skips reload events.
    /// Set during plugin/skill extraction to prevent premature reloads.
    watcher_paused: Arc<AtomicBool>,
    /// Raw content of bundled skills for lazy template loading.
    /// Keyed by skill name, value is the full SKILL.md content from include_str!().
    bundled_raw: HashMap<String, &'static str>,
    /// License keys for sealed .napp files, keyed by artifact_id.
    /// Populated from the license key cache before load_all().
    license_keys: Arc<RwLock<HashMap<String, [u8; 32]>>>,
}

impl Loader {
    pub fn new(installed_dir: PathBuf, user_dir: PathBuf) -> Self {
        // Pre-index bundled skill content for lazy template loading.
        let mut bundled_raw = HashMap::new();
        for (_key, content) in super::bundled::BUNDLED_SKILLS {
            if let Ok(skill) = parse_skill_frontmatter(content.as_bytes()) {
                bundled_raw.insert(skill.name, *content);
            }
        }

        Self {
            user_dir,
            installed_dir,
            learned_dir: None,
            agent_dirs: Vec::new(),
            skills: Arc::new(RwLock::new(HashMap::new())),
            plugin_store: None,
            db_store: None,
            watcher_paused: Arc::new(AtomicBool::new(false)),
            bundled_raw,
            license_keys: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Pause the filesystem watcher (call before extraction).
    pub fn pause_watcher(&self) {
        self.watcher_paused.store(true, Ordering::Relaxed);
    }

    /// Resume the filesystem watcher (call after load_all).
    pub fn resume_watcher(&self) {
        self.watcher_paused.store(false, Ordering::Relaxed);
    }

    /// Set the plugin store for verifying plugin dependencies during load.
    pub fn with_plugin_store(mut self, store: Arc<napp::plugin::PluginStore>) -> Self {
        self.plugin_store = Some(store);
        self
    }

    /// Set the DB store for checking plugin enabled/disabled state during load.
    pub fn with_db_store(mut self, store: Arc<db::Store>) -> Self {
        self.db_store = Some(store);
        self
    }

    /// Set the learned-skills root (<data_dir>/learned/skills/). Enables the
    /// per-employee Learned tier.
    pub fn with_learned_dir(mut self, dir: PathBuf) -> Self {
        self.learned_dir = Some(dir);
        self
    }

    /// Set the employee-package roots (`<data_dir>/nebo/agents/`,
    /// `<data_dir>/user/agents/`). Enables the per-employee Package tier: the
    /// procedures an employee package ships with, visible only to that seat.
    pub fn with_agent_dirs(mut self, dirs: Vec<PathBuf>) -> Self {
        self.agent_dirs = dirs;
        self
    }

    /// Check if a plugin is active (not disabled by user + ready to execute).
    /// A plugin's skills are its documentation, so they load for every
    /// installed plugin the owner has not disabled. Readiness (credentials,
    /// required config) is NOT a gate here: it belongs to exec, where the
    /// plugin tool refuses and says what to connect. Gating on it hid a
    /// plugin's skills from the skill tool the moment it was reinstalled
    /// (caches empty until the first auth probe) and for every plugin whose
    /// accounts are per-employee rather than workspace-wide — the model then
    /// could not even read that an account was needed (2026-09-15).
    fn is_plugin_active(&self, _ps: &napp::plugin::PluginStore, slug: &str) -> bool {
        if let Some(ref db) = self.db_store {
            if let Ok(Some(row)) = db.get_plugin_by_slug(slug) {
                if row.is_enabled == 0 {
                    return false;
                }
            }
        }
        true
    }

    /// Set license keys for sealed .napp decryption (keyed by artifact_id).
    pub async fn set_license_keys(&self, keys: HashMap<String, [u8; 32]>) {
        *self.license_keys.write().await = keys;
    }

    /// Load all skills from embedded (bundled), installed (.napp) and user (loose files) directories.
    /// Loading order: embedded → installed (override by name) → user (override by name)
    /// → employee packages → learned (both keyed per employee, never overriding a global).
    /// After loading, verifies dependencies — skills with missing deps are dropped.
    ///
    /// **Warm start:** Reads a skill manifest index (<50ms) instead of walking the filesystem.
    /// **Cold start:** Full filesystem scan + parallel YAML parsing, then writes manifest for next time.
    pub async fn load_all(&self) -> usize {
        let manifest_path = self.manifest_path();

        // Try warm start from manifest
        if let Some(count) = self.try_warm_load(&manifest_path).await {
            return count;
        }

        // Cold start: full filesystem scan
        let count = self.cold_load_all().await;

        // Write manifest for next warm start
        self.write_manifest(&manifest_path).await;
        count
    }

    /// Force a full reload from the filesystem, discarding the warm-start
    /// manifest first so it gets rebuilt from current on-disk state.
    ///
    /// `load_all()` prefers the cached manifest, which is stale right after a
    /// skill is installed or removed — a warm load would resurrect a
    /// just-deleted skill (its directory is gone but the manifest still lists
    /// it). Mutating paths must call this, not `load_all()`, so the in-memory
    /// set and the manifest both reflect the filesystem.
    pub async fn reload_from_disk(&self) -> usize {
        let _ = std::fs::remove_file(self.manifest_path());
        self.load_all().await
    }

    /// Derive the manifest file path.
    /// Placed inside installed_dir so it lives within the data tree and doesn't
    /// leak into shared temp directories during tests.
    fn manifest_path(&self) -> PathBuf {
        self.installed_dir.join(".skill-manifest.json")
    }

    /// Try loading from a cached manifest. Returns Some(count) on success.
    async fn try_warm_load(&self, manifest_path: &Path) -> Option<usize> {
        let manifest = match SkillManifest::load(manifest_path) {
            Ok(m) => m,
            Err(e) => {
                debug!(error = %e, "no valid skill manifest, falling back to cold load");
                return None;
            }
        };

        // A manifest can outlive the files it points at: a plugin update
        // replaces its versioned directory (0.2.7/ -> 0.2.8/) and every entry
        // under the old path goes dark — the catalog says the skill exists,
        // every load/browse fails, and the model flies blind. If any entry's
        // backing directory is gone, the whole manifest is stale: fall back to
        // a cold scan, which rebuilds it from what is actually on disk.
        let stale = manifest.skills.iter().find(|(_, s)| {
            s.base_dir.as_deref().is_some_and(|d| !d.exists())
                || s.source_path.as_deref().is_some_and(|p| !p.exists())
        });
        if let Some((name, _)) = stale {
            info!(
                skill = %name,
                "skill manifest points at missing files — cold rescanning"
            );
            return None;
        }

        let count = manifest.skills.len();
        let mut loaded = manifest.into_skill_map();

        // Re-inject license keys for sealed skills (keys are runtime-only, not in manifest)
        let keys = self.license_keys.read().await;
        for skill in loaded.values_mut() {
            if let Some(ref napp_path) = skill.napp_path {
                // Extract artifact_id from the napp path (filename without extension)
                if let Some(artifact_id) = napp_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                {
                    if let Some(key) = keys.get(artifact_id) {
                        skill.license_key = Some(*key);
                    }
                }
            }
        }

        *self.skills.write().await = loaded;
        info!(count, "loaded skills from manifest (warm start)");
        Some(count)
    }

    /// Full filesystem scan — cold start path. Walks all directories, parses SKILL.md files.
    async fn cold_load_all(&self) -> usize {
        let mut loaded = HashMap::new();

        // 1. Load embedded bundled skills (frontmatter only — template loaded lazily via get())
        for (name, content) in super::bundled::BUNDLED_SKILLS {
            match parse_skill_frontmatter(content.as_bytes()) {
                Ok(mut skill) => {
                    skill.enabled = true;
                    skill.source = SkillSource::Installed;
                    loaded.insert(skill.name.clone(), skill);
                }
                Err(e) => {
                    warn!(skill = name, error = %e, "failed to parse bundled skill");
                }
            }
        }

        // 2. Load installed skills from extracted directories (override bundled)
        if self.installed_dir.exists() {
            for mut skill in
                load_skills_from_nested_dir(&self.installed_dir, SkillSource::Installed)
            {
                skill.enabled = true;
                loaded.insert(skill.name.clone(), skill);
            }
        }

        // 2.1. Load sealed .napp skills (paid content, read in memory only)
        if self.installed_dir.exists() {
            let keys = self.license_keys.read().await;
            for mut skill in load_sealed_skills(&self.installed_dir, &keys) {
                skill.enabled = true;
                skill.source = SkillSource::Installed;
                loaded.insert(skill.name.clone(), skill);
            }
        }

        // 2.5. Load skills embedded in plugins (override installed by name).
        // Auto-inject the parent plugin slug as a PluginDependency so GWS_BIN etc. get set.
        // Only load skills for plugins the owner has not disabled.
        if let Some(ref ps) = self.plugin_store {
            let plugins_dir = ps.plugins_dir();
            if plugins_dir.exists() {
                if let Ok(entries) = std::fs::read_dir(plugins_dir) {
                    for entry in entries.flatten() {
                        let slug_dir = entry.path();
                        if !slug_dir.is_dir() {
                            continue;
                        }
                        let plugin_slug = match slug_dir.file_name().and_then(|n| n.to_str()) {
                            Some(s) => s.to_string(),
                            None => continue,
                        };
                        if !self.is_plugin_active(ps, &plugin_slug) {
                            continue;
                        }
                        for mut skill in
                            load_skills_from_nested_dir(&slug_dir, SkillSource::Installed)
                        {
                            skill.enabled = true;
                            // Auto-inject parent plugin as dependency if not already declared
                            if !skill.plugins.iter().any(|p| p.name == plugin_slug) {
                                skill.plugins.push(super::skill::PluginDependency {
                                    name: plugin_slug.clone(),
                                    version: "*".to_string(),
                                    optional: false,
                                });
                            }
                            loaded.insert(skill.name.clone(), skill);
                        }
                    }
                }
            }
        }

        // 2.75. Load skills embedded in user plugins (override marketplace plugin skills).
        if let Some(ref ps) = self.plugin_store {
            let user_plugins_dir = ps.user_plugins_dir();
            if user_plugins_dir.exists() {
                if let Ok(entries) = std::fs::read_dir(user_plugins_dir) {
                    for entry in entries.flatten() {
                        let slug_dir = entry.path();
                        if !slug_dir.is_dir() {
                            continue;
                        }
                        let plugin_slug = match slug_dir.file_name().and_then(|n| n.to_str()) {
                            Some(s) => s.to_string(),
                            None => continue,
                        };
                        if !self.is_plugin_active(ps, &plugin_slug) {
                            continue;
                        }
                        for mut skill in
                            load_skills_from_nested_dir(&slug_dir, SkillSource::Installed)
                        {
                            skill.enabled = true;
                            if !skill.plugins.iter().any(|p| p.name == plugin_slug) {
                                skill.plugins.push(super::skill::PluginDependency {
                                    name: plugin_slug.clone(),
                                    version: "*".to_string(),
                                    optional: false,
                                });
                            }
                            loaded.insert(skill.name.clone(), skill);
                        }
                    }
                }
            }
        }

        // 3. Load user skills (override installed by name)
        if self.user_dir.exists() {
            for skill in load_skills_from_dir(&self.user_dir, SkillSource::User) {
                loaded.insert(skill.name.clone(), skill);
            }
        }

        // 4. Load the skills an employee package ships with (per-employee).
        //    Keyed "<agent_id>::<name>" — a seat's own procedure belongs to
        //    that seat and is invisible to every other.
        load_employee_skills(&self.agent_dirs, &mut loaded);

        // 5. Load learned skills (per-employee, written by the self-improvement
        //    loop). Keyed "<agent_id>::<name>" so they never collide with or
        //    override the global roster; read paths filter via visible_to().
        //    Loaded last so a seat's learned refinement of one of its own
        //    packaged procedures wins by name, the same override-by-name rule
        //    the global roots follow.
        if let Some(ref learned_root) = self.learned_dir {
            load_learned_skills(learned_root, &mut loaded);
        }

        // Verify dependencies — skip skills with missing deps or required plugins
        verify_dependencies(&mut loaded, self.plugin_store.as_deref());

        let count = loaded.len();
        *self.skills.write().await = loaded;
        info!(count, installed_dir = %self.installed_dir.display(), user_dir = %self.user_dir.display(), "loaded skills (cold start)");
        count
    }

    /// Write the current in-memory skills to a manifest file for next warm start.
    async fn write_manifest(&self, path: &Path) {
        let skills = self.skills.read().await;
        let hashes = manifest::compute_hashes(&skills);
        let manifest = SkillManifest::from_skill_map(&skills, &hashes);
        if let Err(e) = manifest.save(path) {
            warn!(error = %e, "failed to write skill manifest");
        }
    }

    /// Background verification: check manifest hashes against filesystem,
    /// update stale skills, add new ones, re-verify dependencies, rewrite manifest.
    /// Call this from a background task after warm start.
    pub async fn verify_and_refresh_manifest(&self) {
        let manifest_path = self.manifest_path();
        let manifest = match SkillManifest::load(&manifest_path) {
            Ok(m) => m,
            Err(_) => return, // no manifest to verify
        };

        // Collect plugin directories for scanning
        let mut plugins_dirs = Vec::new();
        if let Some(ref ps) = self.plugin_store {
            let d = ps.plugins_dir().to_path_buf();
            if d.exists() {
                plugins_dirs.push(d);
            }
            let ud = ps.user_plugins_dir().to_path_buf();
            if ud.exists() {
                plugins_dirs.push(ud);
            }
        }

        let (stale, new_paths) = manifest::verify_manifest(
            &manifest,
            &self.installed_dir,
            &self.user_dir,
            &plugins_dirs,
            &self.agent_dirs,
        );

        if stale.is_empty() && new_paths.is_empty() {
            // Re-run verify_dependencies in case plugins changed between runs
            let mut skills = self.skills.write().await;
            verify_dependencies(&mut skills, self.plugin_store.as_deref());
            drop(skills);

            // Rewrite manifest with updated degraded states
            self.write_manifest(&manifest_path).await;
            return;
        }

        info!(
            stale = stale.len(),
            new = new_paths.len(),
            "manifest stale, refreshing changed skills"
        );

        // Remove stale entries and re-parse them from disk
        {
            let mut skills = self.skills.write().await;
            for name in &stale {
                skills.remove(name);
            }
        }

        // Re-parse stale skills from their source paths
        for name in &stale {
            if let Some(entry) = manifest.skills.get(name) {
                if let Some(ref path) = entry.source_path {
                    if let Ok(data) = std::fs::read(path) {
                        if let Ok(mut skill) = parse_skill_frontmatter(&data) {
                            skill.enabled = entry.enabled;
                            skill.source = entry.source;
                            skill.source_path = Some(path.clone());
                            skill.base_dir = entry.base_dir.clone();
                            skill.napp_path = entry.napp_path.clone();
                            // Per-employee entries (a package's own skill, or
                            // a learned one) keep their owner + namespaced key
                            // — a plain-name insert would leak them into the
                            // global roster.
                            skill.owner_agent_id = entry.owner_agent_id.clone();
                            let key = match entry.owner_agent_id.as_deref() {
                                Some(owner) => owner_key(owner, &skill.name),
                                None => skill.name.clone(),
                            };
                            self.skills.write().await.insert(key, skill);
                        }
                    }
                }
            }
        }

        // Parse new skills. A path found under an employee-package root comes
        // back with the owning seat, and keeps the owner + namespaced key —
        // a plain-name insert would publish one seat's procedure to the whole
        // workforce.
        for (md_path, owner) in &new_paths {
            if let Ok(data) = std::fs::read(md_path) {
                if let Ok(mut skill) = parse_skill_frontmatter(&data) {
                    skill.enabled = true;
                    skill.source_path = Some(md_path.clone());
                    skill.base_dir = md_path.parent().map(|p| p.to_path_buf());
                    skill.owner_agent_id = owner.clone();
                    if owner.is_some() {
                        // A package's own procedure is package content — an
                        // update replaces it, so it is read-only like any
                        // other installed artifact.
                        skill.source = SkillSource::Installed;
                    }
                    let key = match owner.as_deref() {
                        Some(owner) => owner_key(owner, &skill.name),
                        None => skill.name.clone(),
                    };
                    self.skills.write().await.insert(key, skill);
                }
            }
        }

        // Re-verify all dependencies
        {
            let mut skills = self.skills.write().await;
            verify_dependencies(&mut skills, self.plugin_store.as_deref());
        }

        // Rewrite manifest
        self.write_manifest(&manifest_path).await;
    }

    /// Get a skill by name, lazily loading the template body if needed.
    /// `agent` scopes the lookup: the agent's own Learned skill wins over a
    /// same-named global (most-specific-first, mirroring load order); other
    /// agents' learned skills are never returned.
    pub async fn get(&self, name: &str, agent: Option<&str>) -> Option<Skill> {
        let skills = self.skills.read().await;
        // Registry keys are the frontmatter `name` (short). Workflow bindings and
        // agent manifests reference skills by qualified name
        // (`@org/skills/<name>`), so resolve that form to its short name here —
        // one lookup rule for every caller, instead of each caller trimming.
        let short = qualified_short_name(name);
        let mut skill = agent
            .and_then(|a| skills.get(&owner_key(a, name)))
            .or_else(|| skills.get(name))
            .or_else(|| short.and_then(|s| skills.get(s)))
            .cloned()?;
        drop(skills);
        if !skill.visible_to(agent) {
            return None;
        }
        if skill.template.is_empty() {
            self.load_template(&mut skill);
        }
        Some(skill)
    }

    /// Populate the template body from disk (source_path), sealed .napp, or bundled content.
    fn load_template(&self, skill: &mut Skill) {
        // Sealed .napp: read SKILL.md from encrypted archive in memory
        if let (Some(napp_path), Some(key)) = (&skill.napp_path, &skill.license_key) {
            match napp::reader::read_sealed_napp_entry(napp_path, "SKILL.md", key) {
                Ok(data) => {
                    if let Ok((_fm, body)) = split_frontmatter(&data) {
                        skill.template = String::from_utf8_lossy(&body).to_string();
                        return;
                    }
                }
                Err(e) => {
                    warn!(skill = %skill.name, error = %e, "failed to read SKILL.md from sealed .napp");
                }
            }
        }

        // Try source_path first (filesystem skills: installed, plugin-embedded, user)
        if let Some(ref path) = skill.source_path {
            if let Ok(data) = std::fs::read(path) {
                if let Ok((_fm, body)) = split_frontmatter(&data) {
                    skill.template = String::from_utf8_lossy(&body).to_string();
                    return;
                }
            }
        }
        // Try bundled content (compiled into binary)
        if let Some(content) = self.bundled_raw.get(&skill.name) {
            if let Ok((_fm, body)) = split_frontmatter(content.as_bytes()) {
                skill.template = String::from_utf8_lossy(&body).to_string();
            }
        }
    }

    /// Load skills from an app's directory (e.g. `<tool_dir>/skills/`).
    /// Each SKILL.md is parsed and registered the same way as plugin-embedded skills.
    /// Returns the names of the loaded skills.
    pub async fn load_app_skills(&self, app_dir: &Path) -> Vec<String> {
        let skills_dir = app_dir.join("skills");
        if !skills_dir.exists() {
            return vec![];
        }
        let app_skills = load_skills_from_nested_dir(&skills_dir, SkillSource::Installed);
        let mut names = Vec::new();
        let mut all = self.skills.write().await;
        for mut skill in app_skills {
            skill.enabled = true;
            names.push(skill.name.clone());
            all.insert(skill.name.clone(), skill);
        }
        drop(all);
        if !names.is_empty() {
            info!(count = names.len(), skills = ?names, "loaded app skills");
        }
        names
    }

    /// Unload skills that were loaded for an app.
    pub async fn unload_skills(&self, names: &[String]) {
        let mut all = self.skills.write().await;
        for name in names {
            all.remove(name);
        }
        drop(all);
        if !names.is_empty() {
            debug!(count = names.len(), "unloaded app skills");
        }
    }

    /// List all skills visible to `agent` (None = main bot: globals only).
    pub async fn list(&self, agent: Option<&str>) -> Vec<Skill> {
        let skills = self.skills.read().await;
        let mut list: Vec<Skill> = skills
            .values()
            .filter(|s| s.visible_to(agent))
            .cloned()
            .collect();
        list.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.name.cmp(&b.name))
        });
        list
    }

    /// Find skills whose triggers match the given message, scoped to `agent`.
    /// Returns up to `max` matches sorted by priority (highest first).
    pub async fn match_triggers(&self, message: &str, max: usize, agent: Option<&str>) -> Vec<Skill> {
        let skills = self.skills.read().await;
        let mut matches: Vec<&Skill> = skills
            .values()
            .filter(|s| s.enabled && s.visible_to(agent) && s.matches_trigger(message))
            .collect();
        matches.sort_by(|a, b| b.priority.cmp(&a.priority));
        matches.truncate(max);
        matches.into_iter().cloned().collect()
    }

    /// The skill listing: every enabled skill visible to `agent`, name → one
    /// line, in Claude Code's shape. A line is the description, with up to
    /// three triggers the name doesn't already say ("pptx" alone doesn't say
    /// "powerpoint"), cut at [`LISTING_LINE_CHARS`]. Past
    /// [`LISTING_BUDGET_CHARS`] the shared skills' lines are shortened evenly
    /// and, past that, dropped to names only; the seat's own skills (its
    /// package's procedures and what it learned, the ones it is employed to
    /// run) keep their whole lines.
    pub async fn listing(&self, agent: Option<&str>) -> std::collections::BTreeMap<String, String> {
        let skills = self.skills.read().await;
        let mut visible: Vec<&Skill> = skills.values().filter(|s| s.enabled && s.visible_to(agent)).collect();
        // A seat's own skill shadows a shared one of the same name.
        visible.sort_by_key(|s| (s.owner_agent_id.is_some(), s.name.clone()));
        let own = |s: &Skill| agent.is_some() && s.owner_agent_id.as_deref() == agent;
        let mut lines: std::collections::BTreeMap<String, (String, bool)> = Default::default();
        for s in visible {
            lines.insert(s.name.clone(), (listing_line(s), own(s)));
        }
        drop(skills);

        let entry_chars = |name: &str, line: &str| name.chars().count() + line.chars().count() + 4;
        let total: usize = lines.iter().map(|(n, (l, _))| entry_chars(n, l) + 1).sum();
        if total > LISTING_BUDGET_CHARS {
            let own_chars: usize = lines.iter().filter(|(_, (_, o))| *o).map(|(n, (l, _))| entry_chars(n, l) + 1).sum();
            let shared: Vec<&String> = lines.iter().filter(|(_, (_, o))| !*o).map(|(n, _)| n).collect();
            let names_chars: usize = shared.iter().map(|n| n.chars().count() + 5).sum();
            let per_line = LISTING_BUDGET_CHARS.saturating_sub(own_chars + names_chars) / shared.len().max(1);
            let cut = |line: &str| -> String {
                if per_line < LISTING_MIN_LINE_CHARS {
                    String::new()
                } else if line.chars().count() > per_line {
                    format!("{}…", line.chars().take(per_line - 1).collect::<String>())
                } else {
                    line.to_string()
                }
            };
            for (line, own) in lines.values_mut() {
                if !*own {
                    *line = cut(line);
                }
            }
        }
        lines.into_iter().map(|(n, (l, _))| (n, l)).collect()
    }

    /// List lightweight summaries of all skills visible to `agent`.
    pub async fn list_summaries(&self, agent: Option<&str>) -> Vec<SkillSummary> {
        let skills = self.skills.read().await;
        let mut list: Vec<SkillSummary> = skills
            .values()
            .filter(|s| s.visible_to(agent))
            .map(|s| s.to_summary())
            .collect();
        list.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.name.cmp(&b.name))
        });
        list
    }

    /// Search skills by query, returning lightweight summaries.
    /// Splits query into tokens and matches each independently (AND logic).
    /// Tokens match against name, description, and triggers with hyphens
    /// treated as word separators.
    /// Shared match+score core for `discover` / `discover_summaries`. Returns the
    /// matching enabled skills (cloned) sorted by relevance (name > triggers >
    /// description); every query token must match at least one field (AND logic).
    ///
    /// Hyphens in the query are normalized to spaces exactly as the skill name is
    /// (`name_lower`), so a query for the exact hyphenated name (e.g.
    /// "nebo-design") tokenizes to ["nebo", "design"] and matches the
    /// de-hyphenated name "nebo design". Without this a `-` stayed in the token
    /// and could never match — discover missed installed skills looked up by
    /// their exact hyphenated name.
    async fn discover_scored(&self, query: &str, agent: Option<&str>) -> Vec<Skill> {
        // Cap on returned matches — discovery is a search, not a dump. Callers
        // surface the top hits; the agent loads the one it wants.
        const MAX_RESULTS: usize = 12;
        // Filler words carry no intent and must NOT be required to match, or a
        // natural phrasing like "check my inbox" fails AND-logic on "my".
        const STOPWORDS: &[&str] = &[
            "a", "an", "and", "the", "to", "of", "for", "in", "on", "my", "me", "i", "you",
            "your", "it", "this", "that", "with", "or", "is", "are", "can", "please", "help",
            "want", "need", "let", "lets", "let's", "do", "get", "show",
        ];
        let skills = self.skills.read().await;
        let raw: Vec<String> = query
            .to_lowercase()
            .replace('-', " ")
            .split_whitespace()
            .map(|t| t.to_string())
            .collect();
        if raw.is_empty() {
            return Vec::new();
        }
        // Meaningful tokens drive matching; if the query is ALL stopwords, fall
        // back to the raw tokens so we never search on an empty set.
        let meaningful: Vec<String> = raw
            .iter()
            .filter(|t| !STOPWORDS.contains(&t.as_str()))
            .cloned()
            .collect();
        let tokens: &[String] = if meaningful.is_empty() { &raw } else { &meaningful };

        let mut matches: Vec<(usize, Skill)> = skills
            .values()
            .filter(|s| s.enabled && s.visible_to(agent))
            .filter_map(|s| {
                let name_lower = s.name.to_lowercase().replace('-', " ");
                let desc_lower = s.description.to_lowercase();
                let triggers_lower: Vec<String> =
                    s.triggers.iter().map(|t| t.to_lowercase()).collect();
                // Ranked recall (OR): a skill is a candidate if ANY token matches
                // a field. Score by field weight (name > triggers > description)
                // plus a coverage bonus for matching more distinct tokens, so the
                // best multi-token match floats to the top without excluding
                // single-token hits the way AND-logic did.
                let mut score: usize = 0;
                let mut matched_tokens = 0usize;
                for tok in tokens {
                    let mut hit = false;
                    if name_lower.contains(tok.as_str()) {
                        score += 3;
                        hit = true;
                    }
                    if triggers_lower.iter().any(|t| t.contains(tok.as_str())) {
                        score += 2;
                        hit = true;
                    }
                    if desc_lower.contains(tok.as_str()) {
                        score += 1;
                        hit = true;
                    }
                    if hit {
                        matched_tokens += 1;
                    }
                }
                if score == 0 {
                    return None;
                }
                // Coverage bonus: matching N distinct tokens outranks a single
                // repeated field hit.
                score += matched_tokens * 2;
                Some((score, s.clone()))
            })
            .collect();
        matches.sort_by(|a, b| b.0.cmp(&a.0));
        matches.truncate(MAX_RESULTS);
        matches.into_iter().map(|(_, s)| s).collect()
    }

    pub async fn discover_summaries(&self, query: &str, agent: Option<&str>) -> Vec<SkillSummary> {
        self.discover_scored(query, agent)
            .await
            .iter()
            .map(|s| s.to_summary())
            .collect()
    }

    /// Search skills by query (name/description/trigger match), sorted by relevance.
    pub async fn discover(&self, query: &str, agent: Option<&str>) -> Vec<Skill> {
        self.discover_scored(query, agent).await
    }

    /// Build a compact plugin inventory for the system prompt.
    ///
    /// Produces a categorized summary of installed connector plugins (~200 tokens)
    /// with instructions to use search for discovery. Provider, hook, and utility
    /// plugins are infrastructure and omitted — they don't need LLM routing.
    pub fn plugin_inventory(&self) -> String {
        let ps = match &self.plugin_store {
            Some(ps) => ps,
            None => return String::new(),
        };
        let installed = ps.list_installed();
        if installed.is_empty() {
            return String::new();
        }

        // Deduplicate slugs and load manifests.
        let mut seen = std::collections::HashSet::new();
        let mut categories: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        let mut uncategorized: Vec<String> = Vec::new();
        let mut total = 0usize;

        for (slug, _, _, _) in &installed {
            if !seen.insert(slug.clone()) {
                continue;
            }
            if !self.is_plugin_active(ps, slug) {
                continue;
            }
            total += 1;
            if let Some(manifest) = ps.get_manifest(slug) {
                let cat = if !manifest.category.is_empty() {
                    manifest.category.to_lowercase()
                } else {
                    String::new()
                };
                if cat.is_empty() {
                    uncategorized.push(slug.clone());
                } else {
                    categories.entry(cat).or_default().push(slug.clone());
                }
            } else {
                uncategorized.push(slug.clone());
            }
        }

        // Build categorized summary. Collapse single-plugin categories into "other"
        // to keep the system prompt concise with many categories.
        let mut category_lines: Vec<String> = Vec::new();
        let mut other_slugs: Vec<String> = uncategorized;
        for (cat, slugs) in &categories {
            if slugs.len() == 1 {
                other_slugs.extend(slugs.iter().cloned());
            } else {
                category_lines.push(format!("{} ({})", cat, slugs.join(", ")));
            }
        }
        if !other_slugs.is_empty() {
            category_lines.push(format!("other ({})", other_slugs.join(", ")));
        }

        let categories_text = if category_lines.is_empty() {
            // Fallback: flat list (no manifests have categories yet)
            let all_slugs: Vec<String> = seen.into_iter().collect();
            all_slugs.join(", ")
        } else {
            category_lines.join(", ")
        };

        format!(
            "## Installed Plugins ({})\n\
             {}\n\n\
             To use a plugin:\n\
             1. Its tool is plugin__<slug>, listed by name; load it with find_tools. find_plugins searches the marketplace when nothing installed fits\n\
             2. use_skill(name: \"<skill name>\") - the plugin's tool names its skills; load the one for the job BEFORE the first command\n\
             3. plugin__<slug> with command: \"<subcommand> +<flags>\"\n\n\
             IMPORTANT: Always read docs (step 2) before your first command on any plugin.\n\
             The command field is CLI args, NOT colon syntax. Never use \"service:method\".\n\n\
             For content with special characters, use args instead of command:\n\
             plugin__<slug> with command: \"docx +create\" and args: {{\"name\": \"report.docx\", \"content\": \"...\"}}\n\n\
             If you already know the plugin, skip to step 2.",
            total,
            categories_text,
        )
    }

    /// Build a focused context section for an agent's required plugins, by
    /// slug (`plugin_tools::plugin_slug_of` reads a job's references).
    /// Lists each required plugin with its description and top skills so the
    /// LLM knows what's available from turn 1 without needing to discover.
    pub fn agent_plugin_context(&self, required_plugins: &[String]) -> String {
        if required_plugins.is_empty() {
            return String::new();
        }
        let ps = match &self.plugin_store {
            Some(ps) => ps,
            None => return String::new(),
        };

        let mut lines = Vec::new();
        for resolved_slug in required_plugins {
            let resolved_slug = resolved_slug.as_str();
            let binary = ps.resolve(resolved_slug, "*");
            if binary.is_none() {
                continue; // Not installed
            }
            let manifest = ps.get_manifest(resolved_slug);

            let desc = manifest
                .as_ref()
                .map(|m| m.description.as_str())
                .unwrap_or("");

            // List skill names from the plugin's skills/ directory
            let skill_names: Vec<String> = if let Some(bin_path) = &binary {
                if let Some(version_dir) = bin_path.parent() {
                    let skills_dir = version_dir.join("skills");
                    if skills_dir.is_dir() {
                        let mut names = Vec::new();
                        if let Ok(entries) = std::fs::read_dir(&skills_dir) {
                            for entry in entries.flatten() {
                                let path = entry.path();
                                if path.is_dir() && path.join("SKILL.md").exists() {
                                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                                        // Strip slug prefix for readability
                                        let short = name
                                            .strip_prefix(&format!("{}-", resolved_slug))
                                            .unwrap_or(name);
                                        names.push(short.to_string());
                                    }
                                }
                            }
                        }
                        names.sort();
                        names
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };

            let mut line = format!("- **{}**", resolved_slug);
            if !desc.is_empty() {
                line.push_str(&format!(" — {}", desc));
            }
            if !skill_names.is_empty() {
                line.push_str(&format!("\n  Skills: {}", skill_names.join(", ")));
            }
            lines.push(line);
        }

        if lines.is_empty() {
            return String::new();
        }

        format!(
            "## Agent Required Plugins\n\
             This agent depends on these plugins. Each one's tool, plugin__<slug>, lists its skills by name; use_skill(name: \"<skill name>\") loads a plugin's usage, and find_skills(query: \"<slug> <task>\") finds the one for a job.\n\n\
             {}\n",
            lines.join("\n")
        )
    }

    /// Start watching for filesystem changes and reload on modification.
    /// Returns a JoinHandle that runs until cancelled.
    pub fn watch(&self) -> tokio::task::JoinHandle<()> {
        let user_dir = self.user_dir.clone();
        let installed_dir = self.installed_dir.clone();
        let learned_dir = self.learned_dir.clone();
        let agent_dirs = self.agent_dirs.clone();
        let skills = self.skills.clone();
        let plugin_store = self.plugin_store.clone();
        let watcher_paused = self.watcher_paused.clone();
        let plugins_dir = plugin_store
            .as_ref()
            .map(|ps| ps.plugins_dir().to_path_buf());
        let user_plugins_dir = plugin_store
            .as_ref()
            .map(|ps| ps.user_plugins_dir().to_path_buf());

        tokio::spawn(async move {
            use notify::{Event, EventKind, RecursiveMode, Watcher};
            use tokio::sync::mpsc;

            // Unbounded + non-blocking send: the callback runs on notify's
            // event-loop thread, and watcher.watch() below round-trips through
            // that same thread. A bounded channel that filled during the watch()
            // setup window blocked the notify thread in blocking_send, which
            // deadlocked watch() → notify → consumer on a single-worker runtime
            // (2026-07-22 cloud incident: 1-vCPU pods froze at end of boot).
            // Same pattern at every notify watcher site in the workspace.
            let (tx, mut rx) = mpsc::unbounded_channel::<notify::Result<Event>>();

            let mut watcher = match notify::RecommendedWatcher::new(
                move |res| {
                    let _ = tx.send(res);
                },
                notify::Config::default().with_poll_interval(std::time::Duration::from_secs(2)),
            ) {
                Ok(w) => w,
                Err(e) => {
                    warn!(error = %e, "failed to create filesystem watcher for skills");
                    return;
                }
            };

            if user_dir.exists() {
                if let Err(e) = watcher.watch(&user_dir, RecursiveMode::Recursive) {
                    warn!(error = %e, dir = %user_dir.display(), "failed to watch user skills dir");
                }
            }

            if installed_dir.exists() {
                if let Err(e) = watcher.watch(&installed_dir, RecursiveMode::Recursive) {
                    warn!(error = %e, dir = %installed_dir.display(), "failed to watch installed skills dir");
                }
            }

            if let Some(ref ldir) = learned_dir {
                if ldir.exists() {
                    if let Err(e) = watcher.watch(ldir, RecursiveMode::Recursive) {
                        warn!(error = %e, dir = %ldir.display(), "failed to watch learned skills dir");
                    }
                }
            }

            // Watch employee-package roots for changes to the skills a package
            // ships with (nebo/agents/<slug>/skills/, user/agents/<slug>/skills/).
            for adir in &agent_dirs {
                if adir.exists() {
                    if let Err(e) = watcher.watch(adir, RecursiveMode::Recursive) {
                        warn!(error = %e, dir = %adir.display(), "failed to watch agents dir for skills");
                    }
                }
            }

            // Watch plugin directory for embedded skill changes
            if let Some(ref pdir) = plugins_dir {
                if pdir.exists() {
                    if let Err(e) = watcher.watch(pdir, RecursiveMode::Recursive) {
                        warn!(error = %e, dir = %pdir.display(), "failed to watch plugins dir for skills");
                    }
                }
            }

            // Watch user plugin directory for embedded skill changes
            if let Some(ref updir) = user_plugins_dir {
                if updir.exists() {
                    if let Err(e) = watcher.watch(updir, RecursiveMode::Recursive) {
                        warn!(error = %e, dir = %updir.display(), "failed to watch user plugins dir for skills");
                    }
                }
            }

            let mut last_reload = std::time::Instant::now();
            let debounce = std::time::Duration::from_secs(1);

            // Storm breaker: a writer inside a watched tree turns the debounce
            // into a permanent ~1 reload/sec loop that pegs a 1-vCPU cloud pod
            // until the liveness probe kills it (2026-07-30 incident: >1h of
            // continuous reloads, bot SIGTERMed mid-install). If reloads keep
            // saturating the window, stop reloading for a cooldown and NAME the
            // paths that keep firing so the writer can be identified and fixed.
            let mut window_start = std::time::Instant::now();
            let mut window_reloads: u32 = 0;
            let mut storm_until: Option<std::time::Instant> = None;
            const STORM_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);
            const STORM_THRESHOLD: u32 = 20;
            const STORM_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(300);

            while let Some(result) = rx.recv().await {
                match result {
                    Ok(event) => {
                        let dominated = matches!(
                            event.kind,
                            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                        );
                        if !dominated {
                            continue;
                        }

                        let relevant = event.paths.iter().any(|p| {
                            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                            if name.eq_ignore_ascii_case("skill.md") || name.ends_with(".napp") {
                                return true;
                            }
                            // Inside an employee package only what lives under
                            // skills/ counts. The rest of a package (AGENT.md,
                            // agent.json, the seat's context) is written by
                            // other subsystems — and one of the resource-dir
                            // names below is "agents", which every path in the
                            // agents tree matches. Without this gate a settings
                            // save would reload every skill in the install.
                            if agent_dirs.iter().any(|root| p.starts_with(root))
                                && !p.ancestors().any(|a| {
                                    a.file_name().and_then(|n| n.to_str()) == Some("skills")
                                })
                            {
                                return false;
                            }
                            // Trigger reload when resource files change
                            p.ancestors().any(|a| {
                                a.file_name()
                                    .and_then(|n| n.to_str())
                                    .map(|n| {
                                        matches!(
                                            n,
                                            "scripts"
                                                | "references"
                                                | "assets"
                                                | "examples"
                                                | "agents"
                                                | "core"
                                        )
                                    })
                                    .unwrap_or(false)
                            })
                        });
                        if !relevant {
                            continue;
                        }

                        // Skip reload while paused (extraction in progress)
                        if watcher_paused.load(Ordering::Relaxed) {
                            continue;
                        }

                        // Coalesce a burst into one reload — but never DROP the
                        // event. An edit that lands inside the debounce window
                        // waits the window out; discarding it left the change on
                        // disk and the old procedure in memory until some
                        // unrelated write happened to trigger the next reload.
                        if let Some(remaining) = debounce.checked_sub(last_reload.elapsed()) {
                            tokio::time::sleep(remaining).await;
                            // Whatever piled up while waiting is covered by the
                            // reload below.
                            while rx.try_recv().is_ok() {}
                        }

                        // Storm breaker: inside a cooldown, drop events outright.
                        if let Some(until) = storm_until {
                            if std::time::Instant::now() < until {
                                continue;
                            }
                            storm_until = None;
                            window_start = std::time::Instant::now();
                            window_reloads = 0;
                            info!("skills watcher: storm cooldown over, resuming reloads");
                        }
                        if window_start.elapsed() > STORM_WINDOW {
                            window_start = std::time::Instant::now();
                            window_reloads = 0;
                        }
                        window_reloads += 1;
                        if window_reloads >= STORM_THRESHOLD {
                            let culprits: Vec<String> = event
                                .paths
                                .iter()
                                .map(|p| p.display().to_string())
                                .collect();
                            warn!(
                                reloads = window_reloads,
                                window_secs = STORM_WINDOW.as_secs(),
                                cooldown_secs = STORM_COOLDOWN.as_secs(),
                                last_event_paths = ?culprits,
                                "skills watcher: reload storm — something keeps writing inside a \
                                 watched tree; pausing reloads (fix the writer, not the watcher)"
                            );
                            storm_until = Some(std::time::Instant::now() + STORM_COOLDOWN);
                            continue;
                        }
                        last_reload = std::time::Instant::now();

                        debug!("skills directory changed, reloading");
                        let mut loaded = HashMap::new();

                        // Reload embedded bundled skills (frontmatter only)
                        for (name, content) in super::bundled::BUNDLED_SKILLS {
                            match parse_skill_frontmatter(content.as_bytes()) {
                                Ok(mut skill) => {
                                    skill.enabled = true;
                                    skill.source = SkillSource::Installed;
                                    loaded.insert(skill.name.clone(), skill);
                                }
                                Err(e) => {
                                    warn!(skill = name, error = %e, "failed to parse bundled skill on reload");
                                }
                            }
                        }

                        if installed_dir.exists() {
                            for mut skill in
                                load_skills_from_nested_dir(&installed_dir, SkillSource::Installed)
                            {
                                skill.enabled = true;
                                loaded.insert(skill.name.clone(), skill);
                            }
                        }

                        // Reload skills embedded in marketplace plugins
                        if let Some(ref pdir) = plugins_dir {
                            if pdir.exists() {
                                if let Ok(entries) = std::fs::read_dir(pdir) {
                                    for entry in entries.flatten() {
                                        let slug_dir = entry.path();
                                        if !slug_dir.is_dir() {
                                            continue;
                                        }
                                        let plugin_slug =
                                            match slug_dir.file_name().and_then(|n| n.to_str()) {
                                                Some(s) => s.to_string(),
                                                None => continue,
                                            };
                                        for mut skill in load_skills_from_nested_dir(
                                            &slug_dir,
                                            SkillSource::Installed,
                                        ) {
                                            skill.enabled = true;
                                            if !skill.plugins.iter().any(|p| p.name == plugin_slug)
                                            {
                                                skill.plugins.push(
                                                    super::skill::PluginDependency {
                                                        name: plugin_slug.clone(),
                                                        version: "*".to_string(),
                                                        optional: false,
                                                    },
                                                );
                                            }
                                            loaded.insert(skill.name.clone(), skill);
                                        }
                                    }
                                }
                            }
                        }

                        // Reload skills embedded in user plugins
                        if let Some(ref updir) = user_plugins_dir {
                            if updir.exists() {
                                if let Ok(entries) = std::fs::read_dir(updir) {
                                    for entry in entries.flatten() {
                                        let slug_dir = entry.path();
                                        if !slug_dir.is_dir() {
                                            continue;
                                        }
                                        let plugin_slug =
                                            match slug_dir.file_name().and_then(|n| n.to_str()) {
                                                Some(s) => s.to_string(),
                                                None => continue,
                                            };
                                        for mut skill in load_skills_from_nested_dir(
                                            &slug_dir,
                                            SkillSource::Installed,
                                        ) {
                                            skill.enabled = true;
                                            if !skill.plugins.iter().any(|p| p.name == plugin_slug)
                                            {
                                                skill.plugins.push(
                                                    super::skill::PluginDependency {
                                                        name: plugin_slug.clone(),
                                                        version: "*".to_string(),
                                                        optional: false,
                                                    },
                                                );
                                            }
                                            loaded.insert(skill.name.clone(), skill);
                                        }
                                    }
                                }
                            }
                        }

                        if user_dir.exists() {
                            for skill in load_skills_from_dir(&user_dir, SkillSource::User) {
                                loaded.insert(skill.name.clone(), skill);
                            }
                        }

                        load_employee_skills(&agent_dirs, &mut loaded);

                        if let Some(ref ldir) = learned_dir {
                            load_learned_skills(ldir, &mut loaded);
                        }

                        verify_dependencies(&mut loaded, plugin_store.as_deref());

                        let count = loaded.len();
                        // Update manifest for next warm start
                        let hashes = manifest::compute_hashes(&loaded);
                        let manifest = SkillManifest::from_skill_map(&loaded, &hashes);
                        let manifest_path = installed_dir.join(".skill-manifest.json");
                        if let Err(e) = manifest.save(&manifest_path) {
                            warn!(error = %e, "failed to write skill manifest after reload");
                        }

                        *skills.write().await = loaded;
                        info!(count, "reloaded skills after filesystem change");
                    }
                    Err(e) => {
                        warn!(error = %e, "filesystem watch error");
                    }
                }
            }
        })
    }

    /// Get the learned skills root (None when the tier is not enabled).
    pub fn learned_dir(&self) -> Option<&Path> {
        self.learned_dir.as_deref()
    }

    /// Get the user skills directory path.
    pub fn user_dir(&self) -> &Path {
        &self.user_dir
    }

    /// Get the installed skills directory path.
    pub fn installed_dir(&self) -> &Path {
        &self.installed_dir
    }

    /// Write a skill to the user skills directory as `{name}/SKILL.md`.
    pub fn write_skill(&self, name: &str, content: &str) -> Result<PathBuf, String> {
        write_skill(&self.user_dir, name, content)
    }

    /// Resolve the path of a user skill by name.
    pub fn resolve_user_skill_path(&self, name: &str) -> Option<PathBuf> {
        resolve_skill_path(&self.user_dir, name)
    }

    /// Expand template variables in a skill's body using runtime context.
    ///
    /// Resolves `${NEBO_SKILL_DIR}`, `${NEBO_DATA_DIR}`, `${NEBO_USER_NAME}`,
    /// `${NEBO_OS}`, `${NEBO_ARCH}`, `${plugin.SLUG_BIN}`, and `${secret.KEY}`.
    pub fn expand_template(&self, skill: &Skill, store: Option<&db::Store>) -> String {
        let ctx = super::expand::build_context(skill, self.plugin_store.as_deref(), store);
        super::expand::expand_variables(&skill.template, &ctx)
    }
}

/// The short name inside a qualified skill reference — `@org/skills/name` →
/// `name` (an optional `@version` suffix is dropped). `None` when the input
/// isn't in qualified form, so plain names never get mangled.
fn qualified_short_name(name: &str) -> Option<&str> {
    let rest = name.strip_prefix('@')?;
    let (_org, after_org) = rest.split_once('/')?;
    let (kind, short) = after_org.split_once('/')?;
    if kind != "skills" || short.is_empty() {
        return None;
    }
    Some(short.split('@').next().unwrap_or(short))
}

/// Map key for a per-employee skill (a package's own procedure, or a learned
/// one): namespaced by owner so per-employee skills can never collide with or
/// shadow the global roster (skill names cannot contain ':', so "::" is
/// unambiguous).
pub(super) fn owner_key(agent_id: &str, name: &str) -> String {
    format!("{}::{}", agent_id, name)
}

/// The agent id the DB row for this package directory carries — the same
/// string a run is scoped by (`agent:<id>:<channel>`), so a packaged skill's
/// owner key matches the scope the runner and the skill tool pass in.
///
/// Reads only manifest.json (and AGENT.md when the manifest has no id), the
/// same precedence the agent loader and the startup DB sync use: manifest
/// `id`, else the display name, else the AGENT.md frontmatter name, else the
/// directory name. Returns None for app packages — an app's skills load
/// through the app lifecycle while the app runs, and must not be registered
/// twice.
fn employee_owner_id(agent_dir: &Path) -> Option<String> {
    let manifest = std::fs::read_to_string(agent_dir.join("manifest.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
    if let Some(ref m) = manifest {
        let kind = m["artifact_type"]
            .as_str()
            .or_else(|| m["type"].as_str())
            .unwrap_or("");
        if kind == "app" {
            return None;
        }
        if let Some(id) = m["id"].as_str().filter(|s| !s.is_empty()) {
            return Some(id.to_string());
        }
    }
    // No id in the manifest: the DB row falls back to the agent's name, so the
    // owner key must fall back the same way.
    let manifest_name = manifest
        .as_ref()
        .and_then(|m| m["name"].as_str().map(String::from))
        .filter(|n| !n.is_empty() && !n.contains('@') && !n.contains('/'));
    if let Some(name) = manifest_name {
        return Some(name);
    }
    let frontmatter_name = std::fs::read_to_string(agent_dir.join("AGENT.md"))
        .ok()
        .and_then(|raw| napp::agent::parse_agent(&raw).ok())
        .map(|def| def.name)
        .filter(|n| !n.is_empty());
    frontmatter_name.or_else(|| {
        agent_dir
            .file_name()
            .and_then(|n| n.to_str())
            .map(String::from)
    })
}

/// Load the procedures employee packages ship with:
/// `<root>/<slug>[/<version>]/skills/<name>/SKILL.md`. Each skill is keyed to
/// the owning seat, so it is available to that employee and invisible to every
/// other — the same rule a learned skill follows.
///
/// `base_dir` stays the skill's real directory inside the package, so a
/// procedure that calls `${NEBO_SKILL_DIR}/scripts/...` still finds its scripts.
///
/// Shared by cold load, the watcher reload and manifest verification — the ONE
/// package-skill pathway.
pub(super) fn load_employee_skills(agent_dirs: &[PathBuf], loaded: &mut HashMap<String, Skill>) {
    for (package, owner) in employee_packages(agent_dirs) {
        let skills_dir = package.join("skills");
        if !skills_dir.is_dir() {
            continue;
        }
        for mut skill in load_skills_from_dir(&skills_dir, SkillSource::Installed) {
            skill.owner_agent_id = Some(owner.clone());
            loaded.insert(owner_key(&owner, &skill.name), skill);
        }
    }
}

/// Every employee package under `agent_dirs`, paired with the agent id that
/// owns it. The ONE place a package directory is turned into a seat identity —
/// the cold load, the watcher and manifest verification all read it from here.
pub(super) fn employee_packages(agent_dirs: &[PathBuf]) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for root in agent_dirs {
        if !root.exists() {
            continue;
        }
        // walk_for_marker stops at the AGENT.md directory, so it finds both
        // install layouts (<slug>/AGENT.md and <slug>/<version>/AGENT.md) and
        // never descends into skills/ itself.
        let mut packages = Vec::new();
        napp::reader::walk_for_marker(root, "AGENT.md", &mut |dir| {
            packages.push(dir.to_path_buf());
        });
        for package in packages {
            if let Some(owner) = employee_owner_id(&package) {
                out.push((package, owner));
            }
        }
    }
    out
}

/// Load the learned tree (<root>/<agent_id>/<skill>/SKILL.md) into `loaded`.
/// Shared by cold load and the watcher reload — the ONE learned-load pathway.
fn load_learned_skills(learned_root: &Path, loaded: &mut HashMap<String, Skill>) {
    if !learned_root.exists() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(learned_root) else {
        return;
    };
    for entry in entries.flatten() {
        let agent_dir = entry.path();
        if !agent_dir.is_dir() {
            continue;
        }
        let Some(agent_id) = entry.file_name().to_str().map(String::from) else {
            continue;
        };
        for mut skill in load_skills_from_dir(&agent_dir, SkillSource::Learned) {
            skill.owner_agent_id = Some(agent_id.clone());
            loaded.insert(owner_key(&agent_id, &skill.name), skill);
        }
    }
}

/// Most characters of one listing line (Claude Code's cap).
const LISTING_LINE_CHARS: usize = 250;

/// The listing's budget in characters (Claude Code's default: 1% of a
/// 200K-token window).
const LISTING_BUDGET_CHARS: usize = 8_000;

/// Below this many characters a shortened line says nothing: the listing
/// drops to names.
const LISTING_MIN_LINE_CHARS: usize = 20;

/// A skill's listing line: its description and the triggers its name
/// doesn't already carry, on one line, cut at [`LISTING_LINE_CHARS`].
fn listing_line(s: &Skill) -> String {
    let desc = s.description.split_whitespace().collect::<Vec<_>>().join(" ");
    let name_lower = s.name.to_lowercase();
    let aliases: Vec<&str> = s
        .triggers
        .iter()
        .map(|t| t.trim())
        .filter(|t| {
            let tl = t.to_lowercase();
            !tl.is_empty() && !name_lower.contains(&tl) && !tl.contains(&name_lower)
        })
        .take(3)
        .collect();
    let line = match (desc.is_empty(), aliases.is_empty()) {
        (_, true) => desc,
        (true, false) => format!("({})", aliases.join(", ")),
        (false, false) => format!("{desc} ({})", aliases.join(", ")),
    };
    if line.chars().count() > LISTING_LINE_CHARS {
        format!("{}…", line.chars().take(LISTING_LINE_CHARS - 1).collect::<String>())
    } else {
        line
    }
}

/// Write a skill file to a directory as `{name}/SKILL.md` per Agent Skills spec.
///
/// If content doesn't have frontmatter, wraps it with minimal `---` frontmatter.
pub fn write_skill(skills_dir: &Path, name: &str, content: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(skills_dir)
        .map_err(|e| format!("failed to create skills dir: {}", e))?;

    let skill_dir = skills_dir.join(name);
    std::fs::create_dir_all(&skill_dir)
        .map_err(|e| format!("failed to create skill dir: {}", e))?;

    let final_content = if content.trim_start().starts_with("---") {
        content.to_string()
    } else {
        format!(
            "---\nname: {}\ndescription: {}\n---\n{}",
            name, name, content
        )
    };

    let path = skill_dir.join("SKILL.md");
    std::fs::write(&path, &final_content)
        .map_err(|e| format!("failed to write SKILL.md: {}", e))?;
    Ok(path)
}

/// Resolve the path of a skill in a directory by name.
pub fn resolve_skill_path(skills_dir: &Path, name: &str) -> Option<PathBuf> {
    let dir_path = skills_dir.join(name);
    if dir_path.is_dir() {
        if let Some(md_path) = find_skill_md(&dir_path) {
            return Some(md_path);
        }
    }
    None
}

/// Load skills from extracted .napp directories in a directory tree.
///
/// Recursively walks the directory looking for SKILL.md marker files
/// (from extracted .napp archives or loose skill dirs).
fn load_skills_from_nested_dir(dir: &Path, source: SkillSource) -> Vec<Skill> {
    // Phase 1: collect all skill directories (fast single-pass walk)
    let mut skill_dirs = Vec::new();
    napp::reader::walk_for_marker(dir, "SKILL.md", &mut |skill_dir| {
        skill_dirs.push(skill_dir.to_path_buf());
    });

    // Phase 2: parse SKILL.md files in parallel
    skill_dirs
        .par_iter()
        .filter_map(|skill_dir| {
            let md_path = find_skill_md(skill_dir)?;
            match std::fs::read(&md_path) {
                Ok(data) => match parse_skill_frontmatter(&data) {
                    Ok(mut skill) => {
                        skill.enabled = true;
                        skill.source = source;
                        skill.source_path = Some(md_path);
                        skill.base_dir = Some(skill_dir.clone());
                        if skill.matches_platform() {
                            Some(skill)
                        } else {
                            debug!(
                                name = %skill.name,
                                platform = ?skill.platform,
                                "skipping installed skill: platform mismatch"
                            );
                            None
                        }
                    }
                    Err(e) => {
                        warn!(path = %skill_dir.display(), error = %e, "failed to parse SKILL.md");
                        None
                    }
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => {
                    warn!(path = %md_path.display(), error = %e, "failed to read SKILL.md");
                    None
                }
            }
        })
        .collect()
}

/// Load skills from sealed .napp files (paid content, decrypted in memory).
///
/// Scans the directory tree for .napp files that have NO fully-extracted sibling
/// directory containing SKILL.md (i.e., the skill content is sealed). Reads
/// SKILL.md frontmatter from the encrypted archive using the provided license keys.
fn load_sealed_skills(dir: &Path, license_keys: &HashMap<String, [u8; 32]>) -> Vec<Skill> {
    let mut skills = Vec::new();
    scan_sealed_napps(dir, license_keys, &mut skills);
    skills
}

/// Recursively scan for sealed .napp files and load their skill frontmatter.
fn scan_sealed_napps(dir: &Path, license_keys: &HashMap<String, [u8; 32]>, out: &mut Vec<Skill>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_sealed_napps(&path, license_keys, out);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("napp") {
            continue;
        }
        // Check if sibling extracted directory has a SKILL.md (free content, already loaded)
        let sibling = path.with_extension("");
        if sibling.is_dir() && find_skill_md(&sibling).is_some() {
            continue; // Free content — already loaded by load_skills_from_nested_dir
        }

        // This is a sealed .napp — try to read manifest for artifact_id
        let artifact_id = match read_artifact_id_from_napp(&path) {
            Some(id) => id,
            None => continue,
        };

        let license_key = match license_keys.get(&artifact_id) {
            Some(k) => k,
            None => {
                debug!(path = %path.display(), artifact_id, "sealed skill: no license key, skipping");
                continue;
            }
        };

        // Read SKILL.md frontmatter from sealed .napp in memory
        match napp::reader::read_sealed_napp_entry(&path, "SKILL.md", license_key) {
            Ok(data) => match parse_skill_frontmatter(&data) {
                Ok(mut skill) => {
                    skill.napp_path = Some(path.clone());
                    skill.license_key = Some(*license_key);
                    // Set base_dir to sibling (partial extraction may have binaries there)
                    if sibling.is_dir() {
                        skill.base_dir = Some(sibling);
                    }
                    if skill.matches_platform() {
                        out.push(skill);
                    }
                }
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "failed to parse sealed SKILL.md");
                }
            },
            Err(e) => {
                warn!(path = %path.display(), error = %e, "failed to read sealed .napp");
            }
        }
    }
}

/// Read the artifact_id from a .napp file's manifest.json (in the outer envelope).
/// Returns None if the manifest can't be read or doesn't have an id field.
fn read_artifact_id_from_napp(napp_path: &Path) -> Option<String> {
    // Try reading manifest.json from the plain (unsigned) outer portion.
    // For sealed .napp files, the envelope header is verified but the payload
    // is encrypted. However, manifest.json may be readable from the sibling
    // extracted directory (partial extraction) or from the .napp before sealing.
    // For now, read from sibling dir if it exists.
    let sibling = napp_path.with_extension("");
    if sibling.is_dir() {
        let manifest = sibling.join("manifest.json");
        if manifest.exists() {
            if let Ok(data) = std::fs::read_to_string(&manifest) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
                    return v["id"].as_str().map(String::from);
                }
            }
        }
    }

    // Fallback: try to derive artifact_id from the directory structure
    // e.g., <data_dir>/nebo/skills/@acme/skill-name/1.0.0.napp
    // The artifact_id would need to come from the manifest inside the sealed .napp.
    // Since we can't read inside without a key, and the key lookup needs the artifact_id,
    // we use the napp filename/path as a key lookup hint.
    // For now, try all available keys (small set in practice).
    None
}

/// Load SKILL.md files from a directory (loose files).
/// Each subdirectory should contain a SKILL.md file.
fn load_skills_from_dir(dir: &Path, source: SkillSource) -> Vec<Skill> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            warn!(error = %e, dir = %dir.display(), "failed to read skills directory");
            return Vec::new();
        }
    };

    // Phase 1: collect subdirectories
    let subdirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();

    // Phase 2: parse SKILL.md files in parallel
    subdirs
        .par_iter()
        .filter_map(|path| {
            // Look for SKILL.md (case-insensitive), then SKILL.md.disabled
            let (md_path, enabled) = if let Some(p) = find_skill_md(path) {
                (p, true)
            } else if let Some(p) = find_skill_md_disabled(path) {
                (p, false)
            } else {
                return None;
            };

            match std::fs::read(&md_path) {
                Ok(data) => match parse_skill_frontmatter(&data) {
                    Ok(mut skill) => {
                        skill.enabled = enabled;
                        skill.source_path = Some(md_path);
                        skill.source = source;
                        skill.base_dir = Some(path.clone());
                        if skill.matches_platform() {
                            Some(skill)
                        } else {
                            debug!(
                                name = %skill.name,
                                platform = ?skill.platform,
                                "skipping skill: platform mismatch"
                            );
                            None
                        }
                    }
                    Err(e) => {
                        warn!(path = %md_path.display(), error = %e, "failed to parse SKILL.md");
                        None
                    }
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => {
                    warn!(path = %md_path.display(), error = %e, "failed to read SKILL.md");
                    None
                }
            }
        })
        .collect()
}

/// Find a SKILL.md file in a directory (case-insensitive).
fn find_skill_md(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.eq_ignore_ascii_case("skill.md") {
            return Some(entry.path());
        }
    }
    None
}

/// Find a SKILL.md.disabled file in a directory (case-insensitive).
fn find_skill_md_disabled(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.eq_ignore_ascii_case("skill.md.disabled") {
            return Some(entry.path());
        }
    }
    None
}

/// Verify skill dependencies — mark skills with missing deps as degraded.
///
/// Skills with unmet dependencies are kept in the registry but marked with
/// `degraded = Some(reason)` so the UI can surface a warning. The skill
/// remains activatable but its missing capabilities will be logged.
fn verify_dependencies(
    loaded: &mut HashMap<String, Skill>,
    plugin_store: Option<&napp::plugin::PluginStore>,
) {
    let names: HashSet<String> = loaded.keys().cloned().collect();
    // Build a version map for requires checking: name -> version string
    let versions: HashMap<String, String> = loaded
        .iter()
        .map(|(name, skill)| (name.clone(), skill.version.clone()))
        .collect();

    for (name, skill) in loaded.iter_mut() {
        let mut reasons = Vec::new();

        // Check inter-skill dependencies (legacy `dependencies` field — bare names)
        for dep in &skill.dependencies {
            if !names.contains(dep) {
                reasons.push(format!("missing dependency: {}", dep));
            }
        }

        // Check skill-to-skill requirements (new `requires` field — with version ranges)
        for req in &skill.requires {
            match versions.get(&req.name) {
                None => {
                    reasons.push(format!("missing required skill: {}", req.name));
                }
                Some(ver_str) => {
                    // Check version compatibility if a range is specified
                    if req.version != "*" && !req.version.is_empty() {
                        if let Ok(req_range) = semver::VersionReq::parse(&req.version) {
                            match semver::Version::parse(ver_str) {
                                Ok(ver) if !req_range.matches(&ver) => {
                                    reasons.push(format!(
                                        "skill {} version {} does not satisfy {}",
                                        req.name, ver_str, req.version
                                    ));
                                }
                                Err(_) => {
                                    // Installed skill has unparseable version — warn but don't fail
                                    warn!(
                                        skill = %name,
                                        required_skill = %req.name,
                                        installed_version = %ver_str,
                                        "cannot verify version: installed skill has unparseable version"
                                    );
                                }
                                _ => {} // version matches
                            }
                        }
                    }
                }
            }
        }

        // Check plugin dependencies (only required ones)
        if let Some(store) = plugin_store {
            for p in &skill.plugins {
                if p.optional {
                    continue;
                }
                if store.resolve(&p.name, &p.version).is_none() {
                    reasons.push(format!(
                        "missing required plugin: {} ({})",
                        p.name, p.version
                    ));
                }
            }
        }

        if !reasons.is_empty() {
            let reason = reasons.join("; ");
            warn!(skill = %name, reason = %reason, "skill degraded: unmet dependencies");
            skill.degraded = Some(reason);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn qualified_short_name_resolves_marketplace_refs_only() {
        assert_eq!(qualified_short_name("@org/skills/my-skill"), Some("my-skill"));
        assert_eq!(qualified_short_name("@org/skills/my-skill@^1.0.0"), Some("my-skill"));
        assert_eq!(qualified_short_name("my-skill"), None);
        assert_eq!(qualified_short_name("@org/agents/my-agent"), None);
        assert_eq!(qualified_short_name("@org/skills/"), None);
    }

    fn create_skill_md(dir: &Path, name: &str, content: &str) {
        let skill_dir = dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), content).unwrap();
    }

    /// Create an extracted skill directory (simulates extracted .napp).
    fn create_skill_extracted(dir: &Path, qualified_name: &str, version: &str, skill_md: &[u8]) {
        let version_dir = dir.join(qualified_name).join(version);
        std::fs::create_dir_all(&version_dir).unwrap();
        std::fs::write(version_dir.join("SKILL.md"), skill_md).unwrap();
        std::fs::write(
            version_dir.join("manifest.json"),
            format!(
                r#"{{"id":"{}","name":"test","version":"{}","artifact_type":"skill"}}"#,
                qualified_name, version
            ),
        )
        .unwrap();
    }

    const BASIC_SKILL: &str = r#"---
name: test-skill
description: A test skill
priority: 5
triggers:
  - test trigger
---

This is the test skill template.
"#;

    const PLATFORM_SKILL: &str = r#"---
name: windows-only
description: Windows only skill
platform:
  - windows
---

Windows specific instructions.
"#;

    #[tokio::test]
    async fn test_load_all() {
        let tmp = TempDir::new().unwrap();
        let installed = TempDir::new().unwrap();
        create_skill_md(tmp.path(), "test-skill", BASIC_SKILL);

        let loader = Loader::new(installed.path().to_path_buf(), tmp.path().to_path_buf());
        let count = loader.load_all().await;
        assert!(
            count >= 1,
            "should load at least the user skill (plus bundled)"
        );

        let skill = loader.get("test-skill", None).await.unwrap();
        assert_eq!(skill.description, "A test skill");
        assert_eq!(skill.priority, 5);
        assert!(skill.template.contains("test skill template"));
        assert_eq!(skill.source, SkillSource::User);
    }

    #[tokio::test]
    async fn test_load_from_extracted() {
        let installed = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();

        create_skill_extracted(
            installed.path(),
            "@acme/skills/test",
            "1.0.0",
            BASIC_SKILL.as_bytes(),
        );

        let loader = Loader::new(installed.path().to_path_buf(), user.path().to_path_buf());
        let count = loader.load_all().await;
        assert!(
            count >= 1,
            "should load at least the installed skill (plus bundled)"
        );

        let skill = loader.get("test-skill", None).await.unwrap();
        assert_eq!(skill.source, SkillSource::Installed);
        assert!(skill.base_dir.is_some());
    }

    #[tokio::test]
    async fn test_user_overrides_installed() {
        let installed = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();

        create_skill_extracted(
            installed.path(),
            "@acme/skills/test",
            "1.0.0",
            BASIC_SKILL.as_bytes(),
        );
        create_skill_md(
            user.path(),
            "test-skill",
            &BASIC_SKILL.replace("A test skill", "User override"),
        );

        let loader = Loader::new(installed.path().to_path_buf(), user.path().to_path_buf());
        loader.load_all().await;

        let skill = loader.get("test-skill", None).await.unwrap();
        assert_eq!(skill.description, "User override");
        assert_eq!(skill.source, SkillSource::User);
    }

    #[tokio::test]
    async fn test_platform_filtering() {
        let installed = TempDir::new().unwrap();
        let tmp = TempDir::new().unwrap();
        create_skill_md(tmp.path(), "windows-only", PLATFORM_SKILL);
        create_skill_md(tmp.path(), "test-skill", BASIC_SKILL);

        let loader = Loader::new(installed.path().to_path_buf(), tmp.path().to_path_buf());
        loader.load_all().await;

        assert!(loader.get("test-skill", None).await.is_some());

        if cfg!(target_os = "windows") {
            assert!(loader.get("windows-only", None).await.is_some());
        } else {
            assert!(loader.get("windows-only", None).await.is_none());
        }
    }

    #[tokio::test]
    async fn test_trigger_matching() {
        let installed = TempDir::new().unwrap();
        let tmp = TempDir::new().unwrap();
        create_skill_md(tmp.path(), "test-skill", BASIC_SKILL);

        let loader = Loader::new(installed.path().to_path_buf(), tmp.path().to_path_buf());
        loader.load_all().await;

        let matches = loader.match_triggers("please test trigger this", 3, None).await;
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "test-skill");

        let no_match = loader.match_triggers("unrelated message", 3, None).await;
        assert!(no_match.is_empty());
    }

    #[tokio::test]
    async fn test_discover_matches_hyphenated_name() {
        // Regression: discover for the exact hyphenated skill name must match.
        // The name is de-hyphenated to "nebo design" internally; the query token
        // must normalize the same way or it never matches (the bug that made
        // discover("nebo-design") miss an installed nebo-design skill).
        const HYPHEN_SKILL: &str = r#"---
name: nebo-design
description: UI/UX and brand design capability
priority: 5
---

Design instructions.
"#;
        let installed = TempDir::new().unwrap();
        let tmp = TempDir::new().unwrap();
        create_skill_md(tmp.path(), "nebo-design", HYPHEN_SKILL);

        let loader = Loader::new(installed.path().to_path_buf(), tmp.path().to_path_buf());
        loader.load_all().await;

        // Assert the exact-named skill is PRESENT (the bug was that it was
        // missing), not an exact count — the loader also pulls in bundled skills,
        // some of which legitimately match these tokens.
        let has = |v: Vec<SkillSummary>| v.iter().any(|s| s.name == "nebo-design");
        assert!(
            has(loader.discover_summaries("nebo-design", None).await),
            "exact hyphenated name should be found (was the bug)"
        );
        assert!(has(loader.discover_summaries("nebo design", None).await));
        assert!(has(loader.discover_summaries("design", None).await));
        assert!(
            !has(loader.discover_summaries("unrelated", None).await),
            "unrelated query must not surface nebo-design"
        );
    }

    #[tokio::test]
    async fn test_discover_recall_ignores_stopwords() {
        // Natural phrasing with filler words must still surface the skill. The
        // old AND-logic failed because EVERY token (incl. "my"/"check") had to
        // match — so "check my inbox" found nothing even with a triage skill.
        const TRIAGE: &str = r#"---
name: gws-gmail-triage
description: Triage your unread email and summarize the inbox
priority: 5
---

Triage instructions.
"#;
        let installed = TempDir::new().unwrap();
        let tmp = TempDir::new().unwrap();
        create_skill_md(tmp.path(), "gws-gmail-triage", TRIAGE);
        let loader = Loader::new(installed.path().to_path_buf(), tmp.path().to_path_buf());
        loader.load_all().await;

        let has = |v: Vec<SkillSummary>| v.iter().any(|s| s.name == "gws-gmail-triage");
        // "check"/"my" are filler; "inbox" carries the intent — must still match.
        assert!(
            has(loader.discover_summaries("check my inbox", None).await),
            "natural phrasing with stopwords should still find the triage skill"
        );
        assert!(has(loader.discover_summaries("inbox", None).await));
        // A query with no meaningful overlap must not surface it.
        assert!(
            !has(loader.discover_summaries("calendar meeting room", None).await),
            "unrelated query must not surface the triage skill"
        );
    }

    /// Create a skill inside a plugin's skills/ subdirectory (simulates embedded plugin skill).
    fn create_plugin_embedded_skill(
        plugins_dir: &Path,
        slug: &str,
        version: &str,
        skill_name: &str,
        skill_md: &str,
    ) {
        let version_dir = plugins_dir.join(slug).join(version);
        let skill_dir = version_dir.join("skills").join(skill_name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), skill_md).unwrap();
        // Write a minimal plugin manifest so PluginStore::is_ready(slug) returns true.
        // The loader only loads skills for active/ready plugins (the is_plugin_active
        // gate), so without a manifest the embedded skill would be skipped.
        let manifest = format!(
            r#"{{"id":"{slug}-id","slug":"{slug}","name":"{slug}","version":"{version}","platforms":{{}}}}"#
        );
        std::fs::write(version_dir.join("plugin.json"), manifest).unwrap();
    }

    #[tokio::test]
    async fn test_user_plugin_skills_loaded() {
        let installed = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();
        let marketplace_plugins = TempDir::new().unwrap();
        let user_plugins = TempDir::new().unwrap();

        let plugin_store = Arc::new(napp::plugin::PluginStore::new(
            marketplace_plugins.path().to_path_buf(),
            user_plugins.path().to_path_buf(),
            None,
        ));

        // Create a skill embedded in a user plugin
        create_plugin_embedded_skill(
            user_plugins.path(),
            "outreach",
            "0.1.0",
            "outreach-email",
            &BASIC_SKILL
                .replace("test-skill", "outreach-email")
                .replace("A test skill", "Send outreach emails"),
        );

        let loader = Loader::new(installed.path().to_path_buf(), user.path().to_path_buf())
            .with_plugin_store(plugin_store);
        let count = loader.load_all().await;
        assert!(
            count >= 1,
            "should load at least the plugin skill (plus bundled)"
        );

        let skill = loader.get("outreach-email", None).await.unwrap();
        assert_eq!(skill.description, "Send outreach emails");
        assert!(skill.enabled);
        // Should auto-inject the parent plugin as a dependency
        assert!(
            skill.plugins.iter().any(|p| p.name == "outreach"),
            "should have outreach plugin dependency"
        );
    }

    #[tokio::test]
    async fn test_user_plugin_skills_override_marketplace_plugin_skills() {
        let installed = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();
        let marketplace_plugins = TempDir::new().unwrap();
        let user_plugins = TempDir::new().unwrap();

        let plugin_store = Arc::new(napp::plugin::PluginStore::new(
            marketplace_plugins.path().to_path_buf(),
            user_plugins.path().to_path_buf(),
            None,
        ));

        // Same skill name in both marketplace and user plugins
        create_plugin_embedded_skill(
            marketplace_plugins.path(),
            "gws",
            "1.0.0",
            "gws-gmail",
            &BASIC_SKILL
                .replace("test-skill", "gws-gmail")
                .replace("A test skill", "Marketplace version"),
        );
        create_plugin_embedded_skill(
            user_plugins.path(),
            "gws",
            "1.0.0",
            "gws-gmail",
            &BASIC_SKILL
                .replace("test-skill", "gws-gmail")
                .replace("A test skill", "User version"),
        );

        let loader = Loader::new(installed.path().to_path_buf(), user.path().to_path_buf())
            .with_plugin_store(plugin_store);
        loader.load_all().await;

        let skill = loader.get("gws-gmail", None).await.unwrap();
        assert_eq!(
            skill.description, "User version",
            "user plugin skills should override marketplace plugin skills"
        );
    }

    /// Build an employee package on disk: AGENT.md + manifest.json + one
    /// skill at <pkg>/skills/<skill>/SKILL.md, with a script beside it.
    fn create_employee_package(
        agents_root: &Path,
        slug: &str,
        agent_id: &str,
        skill_name: &str,
        skill_md: &str,
    ) -> PathBuf {
        let pkg = agents_root.join(slug);
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(
            pkg.join("AGENT.md"),
            format!("---\nname: {}\n---\n# {}\n", slug, slug),
        )
        .unwrap();
        std::fs::write(
            pkg.join("manifest.json"),
            format!(
                r#"{{"id":"{}","name":"@acme/agents/{}","type":"agent","version":"1.0.0"}}"#,
                agent_id, slug
            ),
        )
        .unwrap();
        let skill_dir = pkg.join("skills").join(skill_name);
        std::fs::create_dir_all(skill_dir.join("scripts")).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), skill_md).unwrap();
        std::fs::write(skill_dir.join("scripts").join("run.py"), "print('ok')\n").unwrap();
        pkg
    }

    fn package_skill_md(name: &str, description: &str) -> String {
        format!(
            "---\nname: {}\ndescription: {}\n---\n\nRun python3 ${{NEBO_SKILL_DIR}}/scripts/run.py\n",
            name, description
        )
    }

    #[tokio::test]
    async fn test_employee_package_skill_belongs_to_its_seat() {
        let installed = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();
        let agents = TempDir::new().unwrap();

        // Two seats ship a procedure under the SAME name — 14 of the shipped
        // packages do exactly this (project-conventions). Each seat must get
        // its own, and nobody else's.
        create_employee_package(
            agents.path(),
            "copywriter",
            "copywriter",
            "project-conventions",
            &package_skill_md("project-conventions", "How the copywriter works"),
        );
        create_employee_package(
            agents.path(),
            "closer",
            "closer",
            "project-conventions",
            &package_skill_md("project-conventions", "How the closer works"),
        );

        let loader = Loader::new(installed.path().to_path_buf(), user.path().to_path_buf())
            .with_agent_dirs(vec![agents.path().to_path_buf()]);
        loader.load_all().await;

        // Each seat sees its own.
        let mine = loader
            .get("project-conventions", Some("copywriter"))
            .await
            .expect("the seat must see the procedure its own package ships");
        assert_eq!(mine.description, "How the copywriter works");
        assert_eq!(mine.owner_agent_id.as_deref(), Some("copywriter"));
        assert_eq!(mine.source, SkillSource::Installed);

        let theirs = loader
            .get("project-conventions", Some("closer"))
            .await
            .unwrap();
        assert_eq!(
            theirs.description, "How the closer works",
            "a same-named procedure must not leak across seats"
        );

        // Invisible to the main bot and to a third seat.
        assert!(
            loader.get("project-conventions", None).await.is_none(),
            "a seat's own procedure must not be on the shared roster"
        );
        assert!(
            loader
                .get("project-conventions", Some("stranger"))
                .await
                .is_none(),
            "another employee must not see it"
        );
        assert!(
            !loader
                .list(None)
                .await
                .iter()
                .any(|s| s.name == "project-conventions"),
            "list() with no scope must not surface a seat's own procedure"
        );
        assert!(
            loader
                .list(Some("copywriter"))
                .await
                .iter()
                .any(|s| s.name == "project-conventions"),
            "the owning seat's list must include it"
        );

        // The template loads, and the scripts beside it still resolve from the
        // skill's own directory inside the package.
        assert!(mine.template.contains("scripts/run.py"));
        let base = mine.base_dir.clone().expect("base_dir");
        assert!(
            base.join("scripts").join("run.py").exists(),
            "relative scripts must still be found under base_dir"
        );
        assert!(base.starts_with(agents.path()), "base_dir stays in the package");
        let expanded = loader.expand_template(&mine, None);
        assert!(
            expanded.contains(base.join("scripts").join("run.py").to_str().unwrap()),
            "${{NEBO_SKILL_DIR}} must expand to the package's skill directory"
        );

        // The skill listing carries it for its own seat only.
        assert!(loader.listing(Some("copywriter")).await.contains_key("project-conventions"));
        assert!(!loader.listing(None).await.contains_key("project-conventions"));
    }

    #[tokio::test]
    async fn test_employee_package_skill_survives_warm_start() {
        let installed = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();
        let agents = TempDir::new().unwrap();
        create_employee_package(
            agents.path(),
            "copywriter",
            "copywriter",
            "copy-brief",
            &package_skill_md("copy-brief", "Test a brief"),
        );

        // Cold load writes the manifest...
        let cold = Loader::new(installed.path().to_path_buf(), user.path().to_path_buf())
            .with_agent_dirs(vec![agents.path().to_path_buf()]);
        cold.load_all().await;
        assert!(installed.path().join(".skill-manifest.json").exists());

        // ...and a second loader warm-starts from it with scoping intact.
        let warm = Loader::new(installed.path().to_path_buf(), user.path().to_path_buf())
            .with_agent_dirs(vec![agents.path().to_path_buf()]);
        warm.load_all().await;
        let mine = warm
            .get("copy-brief", Some("copywriter"))
            .await
            .expect("warm start must keep the seat's own procedure");
        assert_eq!(mine.owner_agent_id.as_deref(), Some("copywriter"));
        assert!(
            mine.template.contains("scripts/run.py"),
            "the body must still load from disk after a warm start"
        );
        assert!(warm.get("copy-brief", None).await.is_none());
    }

    #[tokio::test]
    async fn test_employee_package_skill_watcher_picks_up_a_change() {
        let installed = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();
        let agents = TempDir::new().unwrap();
        let pkg = create_employee_package(
            agents.path(),
            "copywriter",
            "copywriter",
            "copy-brief",
            &package_skill_md("copy-brief", "First wording"),
        );

        let loader = Arc::new(
            Loader::new(installed.path().to_path_buf(), user.path().to_path_buf())
                .with_agent_dirs(vec![agents.path().to_path_buf()]),
        );
        loader.load_all().await;
        let handle = loader.watch();

        // No fixed sleep anywhere: watch() hands back no "armed" signal (the
        // agent loader's hands back an event receiver, this one does not), so
        // the test re-touches the file and waits on the observable result
        // instead of guessing how long arming takes. The first touch the armed
        // watcher sees is enough — the debounce coalesces it into a reload
        // rather than dropping it — and the loop exits the moment the loader
        // reports the new wording.
        let skill_md = pkg.join("skills").join("copy-brief").join("SKILL.md");
        let mut seen = String::new();
        for _ in 0..80 {
            std::fs::write(&skill_md, package_skill_md("copy-brief", "Second wording")).unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(125)).await;
            if let Some(skill) = loader.get("copy-brief", Some("copywriter")).await {
                seen = skill.description.clone();
                if seen == "Second wording" {
                    break;
                }
            }
        }
        handle.abort();
        assert_eq!(
            seen, "Second wording",
            "the watcher must reload a package's own procedure when it changes"
        );
    }

    #[tokio::test]
    async fn test_listing_lines_are_one_line_descriptions() {
        let installed = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();
        let agents = TempDir::new().unwrap();
        create_employee_package(
            agents.path(),
            "copywriter",
            "copywriter",
            "copy-brief",
            &package_skill_md("copy-brief", "Test a brief"),
        );
        let loader = Loader::new(installed.path().to_path_buf(), user.path().to_path_buf())
            .with_agent_dirs(vec![agents.path().to_path_buf()]);
        loader.load_all().await;

        let seat = loader.listing(Some("copywriter")).await;
        assert_eq!(seat.get("copy-brief").map(String::as_str), Some("Test a brief"));
        let skill = |name: &str, desc: &str, triggers: &str| {
            super::super::skill::parse_skill_md(
                format!("---\nname: {name}\ndescription: \"{desc}\"\ntriggers: [{triggers}]\n---\nbody\n").as_bytes(),
            )
            .unwrap()
        };
        let long = skill("deck", &format!("Build a\\n deck {}", "x".repeat(400)), "powerpoint, deck");
        let line = listing_line(&long);
        assert!(line.starts_with("Build a deck "), "{line}");
        assert_eq!(line.chars().count(), LISTING_LINE_CHARS);
        assert!(line.ends_with('…'));
        let short = skill("pptx", "Slides", "powerpoint, pptx");
        assert_eq!(listing_line(&short), "Slides (powerpoint)");
    }

    /// Past the budget the shared lines shorten, then drop to names; the
    /// seat's own lines stay whole.
    #[tokio::test]
    async fn test_listing_keeps_a_seats_own_lines_past_the_budget() {
        let installed = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();
        let agents = TempDir::new().unwrap();
        for i in 0..60 {
            let name = format!("shared-skill-{i:02}");
            let dir = user.path().join(&name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: {}\n---\nbody\n", "d".repeat(240)),
            )
            .unwrap();
        }
        create_employee_package(
            agents.path(),
            "copywriter",
            "copywriter",
            "copy-brief",
            &package_skill_md("copy-brief", &"o".repeat(240)),
        );
        let loader = Loader::new(installed.path().to_path_buf(), user.path().to_path_buf())
            .with_agent_dirs(vec![agents.path().to_path_buf()]);
        loader.load_all().await;

        let seat = loader.listing(Some("copywriter")).await;
        assert_eq!(seat["copy-brief"], "o".repeat(240), "the seat's own line is whole");
        let shared = &seat["shared-skill-00"];
        assert!(shared.chars().count() < 240 && shared.ends_with('…'), "{shared}");
        let chars: usize = seat.iter().map(|(n, l)| n.len() + l.chars().count() + 5).sum();
        assert!(chars <= LISTING_BUDGET_CHARS + 300, "{chars}");
    }

    #[tokio::test]
    async fn test_listing_stays_small_with_a_whole_company() {
        // 48 employees, 4 procedures each, every seat using the same four
        // names. The shared catalog that goes into every prompt must not grow
        // with the workforce; a seat only ever pays for its own four.
        let installed = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();
        let agents = TempDir::new().unwrap();
        // Names no bundled skill's own text can contain, so the assertions
        // below test presence in the catalog and not a chance substring.
        let names = [
            "seatproc-intake",
            "seatproc-review",
            "seatproc-handoff",
            "seatproc-report",
        ];
        let description = "x".repeat(300);
        for seat in 0..48 {
            let slug = format!("seat-{}", seat);
            for name in names {
                create_employee_package(
                    agents.path(),
                    &slug,
                    &slug,
                    name,
                    &package_skill_md(name, &description),
                );
            }
        }

        let loader = Loader::new(installed.path().to_path_buf(), user.path().to_path_buf())
            .with_agent_dirs(vec![agents.path().to_path_buf()]);
        loader.load_all().await;

        let shared = loader.listing(None).await;
        for name in names {
            assert!(!shared.contains_key(name), "{name} must not reach the shared listing");
        }

        let seat = loader.listing(Some("seat-7")).await;
        for name in names {
            assert!(seat.contains_key(name), "the seat must see its own {name}");
        }
        assert_eq!(seat.len() - shared.len(), names.len(), "a seat pays for its own four, never the workforce's");
    }

    #[tokio::test]
    async fn test_list_sorted_by_priority() {
        let installed = TempDir::new().unwrap();
        let tmp = TempDir::new().unwrap();
        create_skill_md(
            tmp.path(),
            "low",
            &BASIC_SKILL
                .replace("test-skill", "low")
                .replace("priority: 5", "priority: 1"),
        );
        create_skill_md(
            tmp.path(),
            "high",
            &BASIC_SKILL
                .replace("test-skill", "high")
                .replace("priority: 5", "priority: 100"),
        );

        let loader = Loader::new(installed.path().to_path_buf(), tmp.path().to_path_buf());
        loader.load_all().await;

        let list = loader.list(None).await;
        assert!(list.len() >= 2, "should have at least the two user skills");
        // Priority 100 should sort before priority 1 (and before bundled defaults at priority 5)
        assert_eq!(list[0].name, "high");
    }
}
