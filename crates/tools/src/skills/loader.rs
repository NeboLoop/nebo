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
/// (bundled), sealed .napp archives (nebo/skills/) and loose files (user/skills/).
pub struct Loader {
    /// User skills directory (e.g. <data_dir>/user/skills/).
    user_dir: PathBuf,
    /// Installed (marketplace) skills directory (e.g. <data_dir>/nebo/skills/).
    installed_dir: PathBuf,
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
    /// Pre-built compact catalog string, rebuilt on load_all() / watcher reload.
    /// Names-only format (like Claude Code's deferred tool listing).
    cached_catalog: Arc<RwLock<String>>,
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
            skills: Arc::new(RwLock::new(HashMap::new())),
            plugin_store: None,
            db_store: None,
            watcher_paused: Arc::new(AtomicBool::new(false)),
            bundled_raw,
            cached_catalog: Arc::new(RwLock::new(String::new())),
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

    /// Check if a plugin is active (not disabled by user + ready to execute).
    fn is_plugin_active(&self, ps: &napp::plugin::PluginStore, slug: &str) -> bool {
        if let Some(ref db) = self.db_store {
            if let Ok(Some(row)) = db.get_plugin_by_slug(slug) {
                if row.is_enabled == 0 {
                    return false;
                }
            }
        }
        ps.is_ready(slug)
    }

    /// Set license keys for sealed .napp decryption (keyed by artifact_id).
    pub async fn set_license_keys(&self, keys: HashMap<String, [u8; 32]>) {
        *self.license_keys.write().await = keys;
    }

    /// Load all skills from embedded (bundled), installed (.napp) and user (loose files) directories.
    /// Loading order: embedded → installed (override by name) → user (override by name).
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

        let catalog = build_catalog_string(&loaded);
        *self.skills.write().await = loaded;
        *self.cached_catalog.write().await = catalog;
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
        // Only load skills for active plugins (not disabled + ready).
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

        // Verify dependencies — skip skills with missing deps or required plugins
        verify_dependencies(&mut loaded, self.plugin_store.as_deref());

        let count = loaded.len();
        // Rebuild cached catalog before storing (names-only, like Claude Code deferred tools)
        let catalog = build_catalog_string(&loaded);
        *self.skills.write().await = loaded;
        *self.cached_catalog.write().await = catalog;
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
        );

        if stale.is_empty() && new_paths.is_empty() {
            // Re-run verify_dependencies in case plugins changed between runs
            let mut skills = self.skills.write().await;
            verify_dependencies(&mut skills, self.plugin_store.as_deref());
            let catalog = build_catalog_string(&skills);
            drop(skills);
            *self.cached_catalog.write().await = catalog;

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
                            self.skills.write().await.insert(skill.name.clone(), skill);
                        }
                    }
                }
            }
        }

        // Parse new skills
        for md_path in &new_paths {
            if let Ok(data) = std::fs::read(md_path) {
                if let Ok(mut skill) = parse_skill_frontmatter(&data) {
                    skill.enabled = true;
                    skill.source_path = Some(md_path.clone());
                    skill.base_dir = md_path.parent().map(|p| p.to_path_buf());
                    self.skills.write().await.insert(skill.name.clone(), skill);
                }
            }
        }

        // Re-verify all dependencies and rebuild catalog
        {
            let mut skills = self.skills.write().await;
            verify_dependencies(&mut skills, self.plugin_store.as_deref());
            let catalog = build_catalog_string(&skills);
            drop(skills);
            *self.cached_catalog.write().await = catalog;
        }

        // Rewrite manifest
        self.write_manifest(&manifest_path).await;
    }

    /// Get a skill by name, lazily loading the template body if needed.
    pub async fn get(&self, name: &str) -> Option<Skill> {
        let mut skill = self.skills.read().await.get(name).cloned()?;
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
        if !names.is_empty() {
            // Rebuild catalog with new skills
            let catalog = build_catalog_string(&all);
            drop(all);
            *self.cached_catalog.write().await = catalog;
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
        if !names.is_empty() {
            let catalog = build_catalog_string(&all);
            drop(all);
            *self.cached_catalog.write().await = catalog;
            debug!(count = names.len(), "unloaded app skills");
        }
    }

    /// List all loaded skills.
    pub async fn list(&self) -> Vec<Skill> {
        let skills = self.skills.read().await;
        let mut list: Vec<Skill> = skills.values().cloned().collect();
        list.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.name.cmp(&b.name))
        });
        list
    }

    /// Find skills whose triggers match the given message.
    /// Returns up to `max` matches sorted by priority (highest first).
    pub async fn match_triggers(&self, message: &str, max: usize) -> Vec<Skill> {
        let skills = self.skills.read().await;
        let mut matches: Vec<&Skill> = skills
            .values()
            .filter(|s| s.enabled && s.matches_trigger(message))
            .collect();
        matches.sort_by(|a, b| b.priority.cmp(&a.priority));
        matches.truncate(max);
        matches.into_iter().cloned().collect()
    }

    /// Return the pre-built compact skill catalog for the system prompt.
    /// Names-only format (rebuilt on load_all / watcher reload).
    pub async fn compact_catalog(&self) -> String {
        self.cached_catalog.read().await.clone()
    }

    /// List lightweight summaries of all loaded skills.
    pub async fn list_summaries(&self) -> Vec<SkillSummary> {
        let skills = self.skills.read().await;
        let mut list: Vec<SkillSummary> = skills.values().map(|s| s.to_summary()).collect();
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
    async fn discover_scored(&self, query: &str) -> Vec<Skill> {
        let skills = self.skills.read().await;
        let tokens: Vec<String> = query
            .to_lowercase()
            .replace('-', " ")
            .split_whitespace()
            .map(|t| t.to_string())
            .collect();
        if tokens.is_empty() {
            return Vec::new();
        }
        let mut matches: Vec<(usize, Skill)> = skills
            .values()
            .filter(|s| s.enabled)
            .filter_map(|s| {
                let name_lower = s.name.to_lowercase().replace('-', " ");
                let desc_lower = s.description.to_lowercase();
                let triggers_lower: Vec<String> =
                    s.triggers.iter().map(|t| t.to_lowercase()).collect();
                // Every token must match at least one field.
                let all_match = tokens.iter().all(|tok| {
                    name_lower.contains(tok.as_str())
                        || desc_lower.contains(tok.as_str())
                        || triggers_lower.iter().any(|t| t.contains(tok.as_str()))
                });
                if !all_match {
                    return None;
                }
                // Score: name matches highest, then triggers, then description.
                let mut score: usize = 0;
                for tok in &tokens {
                    if name_lower.contains(tok.as_str()) {
                        score += 3;
                    }
                    if triggers_lower.iter().any(|t| t.contains(tok.as_str())) {
                        score += 2;
                    }
                    if desc_lower.contains(tok.as_str()) {
                        score += 1;
                    }
                }
                Some((score, s.clone()))
            })
            .collect();
        matches.sort_by(|a, b| b.0.cmp(&a.0));
        matches.into_iter().map(|(_, s)| s).collect()
    }

    pub async fn discover_summaries(&self, query: &str) -> Vec<SkillSummary> {
        self.discover_scored(query)
            .await
            .iter()
            .map(|s| s.to_summary())
            .collect()
    }

    /// Search skills by query (name/description/trigger match), sorted by relevance.
    pub async fn discover(&self, query: &str) -> Vec<Skill> {
        self.discover_scored(query).await
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
             1. plugin(action: \"search\", query: \"what you need\") — find matching plugins\n\
             2. plugin(action: \"skills\", resource: \"<slug>\", query: \"what you need\") — search a plugin's skills\n\
             3. plugin(resource: \"<slug>\", action: \"help\", topic: \"<skill>\") — read docs BEFORE exec\n\
             4. plugin(resource: \"<slug>\", action: \"exec\", command: \"<subcommand> +<flags>\")\n\n\
             IMPORTANT: Always read docs (step 3) before your first exec of any plugin skill.\n\
             The command field is CLI args — NOT colon syntax. Never use \"service:method\".\n\n\
             For content with special characters, use args instead of command:\n\
             plugin(resource: \"<slug>\", action: \"exec\", command: \"docx +create\", args: {{\"name\": \"report.docx\", \"content\": \"...\"}})\n\n\
             If you already know the plugin slug, skip to step 2.",
            total,
            categories_text,
        )
    }

    /// Build a focused context section for an agent's required plugins.
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
        for plugin_ref in required_plugins {
            // Resolve the slug — may be an install code or slug directly
            let slug = plugin_ref
                .split('-')
                .last()
                .unwrap_or(plugin_ref.as_str());

            // Try both the raw reference and the extracted slug
            let manifest = ps.get_manifest(plugin_ref)
                .or_else(|| ps.get_manifest(slug));
            let binary = ps.resolve(plugin_ref, "*")
                .or_else(|| ps.resolve(slug, "*"));

            let resolved_slug = if ps.resolve(plugin_ref, "*").is_some() {
                plugin_ref.as_str()
            } else if ps.resolve(slug, "*").is_some() {
                slug
            } else {
                continue; // Not installed
            };

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
             This agent depends on these plugins. Use plugin(action: \"skills\", resource: \"<slug>\", query: \"...\") to find the right skill for a task.\n\n\
             {}\n",
            lines.join("\n")
        )
    }

    /// Start watching for filesystem changes and reload on modification.
    /// Returns a JoinHandle that runs until cancelled.
    pub fn watch(&self) -> tokio::task::JoinHandle<()> {
        let user_dir = self.user_dir.clone();
        let installed_dir = self.installed_dir.clone();
        let skills = self.skills.clone();
        let cached_catalog = self.cached_catalog.clone();
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

            let (tx, mut rx) = mpsc::channel::<notify::Result<Event>>(32);

            let mut watcher = match notify::RecommendedWatcher::new(
                move |res| {
                    let _ = tx.blocking_send(res);
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
                            name.eq_ignore_ascii_case("skill.md")
                                || name.ends_with(".napp")
                                // Trigger reload when resource files change
                                || p.ancestors().any(|a| {
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

                        if last_reload.elapsed() < debounce {
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

                        verify_dependencies(&mut loaded, plugin_store.as_deref());

                        let count = loaded.len();
                        let catalog = build_catalog_string(&loaded);

                        // Update manifest for next warm start
                        let hashes = manifest::compute_hashes(&loaded);
                        let manifest = SkillManifest::from_skill_map(&loaded, &hashes);
                        let manifest_path = installed_dir.join(".skill-manifest.json");
                        if let Err(e) = manifest.save(&manifest_path) {
                            warn!(error = %e, "failed to write skill manifest after reload");
                        }

                        *skills.write().await = loaded;
                        *cached_catalog.write().await = catalog;
                        info!(count, "reloaded skills after filesystem change");
                    }
                    Err(e) => {
                        warn!(error = %e, "filesystem watch error");
                    }
                }
            }
        })
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

/// Build a compact catalog header for the system prompt.
///
/// Does NOT list all skill names — the model uses skill(action: "discover")
/// to search on demand. Only shows the count so the model knows skills exist.
fn build_catalog_string(skills: &HashMap<String, Skill>) -> String {
    let count = skills.values().filter(|s| s.enabled).count();

    if count == 0 {
        return String::new();
    }

    format!(
        "## Available Skills ({} installed)\n\n\
         Use skill(action: \"discover\", query: \"...\") to find skills by capability.\n\
         Use skill(action: \"help\", name: \"...\") for full instructions before using a skill.",
        count
    )
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
    // e.g., ~/.nebo/data/nebo/skills/@acme/skill-name/1.0.0.napp
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

        let skill = loader.get("test-skill").await.unwrap();
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

        let skill = loader.get("test-skill").await.unwrap();
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

        let skill = loader.get("test-skill").await.unwrap();
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

        assert!(loader.get("test-skill").await.is_some());

        if cfg!(target_os = "windows") {
            assert!(loader.get("windows-only").await.is_some());
        } else {
            assert!(loader.get("windows-only").await.is_none());
        }
    }

    #[tokio::test]
    async fn test_trigger_matching() {
        let installed = TempDir::new().unwrap();
        let tmp = TempDir::new().unwrap();
        create_skill_md(tmp.path(), "test-skill", BASIC_SKILL);

        let loader = Loader::new(installed.path().to_path_buf(), tmp.path().to_path_buf());
        loader.load_all().await;

        let matches = loader.match_triggers("please test trigger this", 3).await;
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "test-skill");

        let no_match = loader.match_triggers("unrelated message", 3).await;
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
            has(loader.discover_summaries("nebo-design").await),
            "exact hyphenated name should be found (was the bug)"
        );
        assert!(has(loader.discover_summaries("nebo design").await));
        assert!(has(loader.discover_summaries("design").await));
        assert!(
            !has(loader.discover_summaries("unrelated").await),
            "unrelated query must not surface nebo-design"
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

        let skill = loader.get("outreach-email").await.unwrap();
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

        let skill = loader.get("gws-gmail").await.unwrap();
        assert_eq!(
            skill.description, "User version",
            "user plugin skills should override marketplace plugin skills"
        );
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

        let list = loader.list().await;
        assert!(list.len() >= 2, "should have at least the two user skills");
        // Priority 100 should sort before priority 1 (and before bundled defaults at priority 5)
        assert_eq!(list[0].name, "high");
    }
}
