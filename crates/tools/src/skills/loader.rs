use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::skill::{SkillSummary, parse_skill_frontmatter, Skill, SkillSource, split_frontmatter};

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
    /// When true, the filesystem watcher skips reload events.
    /// Set during plugin/skill extraction to prevent premature reloads.
    watcher_paused: Arc<AtomicBool>,
    /// Raw content of bundled skills for lazy template loading.
    /// Keyed by skill name, value is the full SKILL.md content from include_str!().
    bundled_raw: HashMap<String, &'static str>,
    /// Pre-built compact catalog string, rebuilt on load_all() / watcher reload.
    /// Names-only format (like Claude Code's deferred tool listing).
    cached_catalog: Arc<RwLock<String>>,
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
            watcher_paused: Arc::new(AtomicBool::new(false)),
            bundled_raw,
            cached_catalog: Arc::new(RwLock::new(String::new())),
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

    /// Load all skills from embedded (bundled), installed (.napp) and user (loose files) directories.
    /// Loading order: embedded → installed (override by name) → user (override by name).
    /// After loading, verifies dependencies — skills with missing deps are dropped.
    pub async fn load_all(&self) -> usize {
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
            for mut skill in load_skills_from_nested_dir(&self.installed_dir, SkillSource::Installed) {
                skill.enabled = true;
                loaded.insert(skill.name.clone(), skill);
            }
        }

        // 2.5. Load skills embedded in plugins (override installed by name).
        // Auto-inject the parent plugin slug as a PluginDependency so GWS_BIN etc. get set.
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
                        for mut skill in load_skills_from_nested_dir(&slug_dir, SkillSource::Installed) {
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
                        for mut skill in load_skills_from_nested_dir(&slug_dir, SkillSource::Installed) {
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
        info!(count, installed_dir = %self.installed_dir.display(), user_dir = %self.user_dir.display(), "loaded skills");
        count
    }

    /// Get a skill by name, lazily loading the template body if needed.
    pub async fn get(&self, name: &str) -> Option<Skill> {
        let mut skill = self.skills.read().await.get(name).cloned()?;
        if skill.template.is_empty() {
            self.load_template(&mut skill);
        }
        Some(skill)
    }

    /// Populate the template body from disk (source_path) or bundled content.
    fn load_template(&self, skill: &mut Skill) {
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
    pub async fn discover_summaries(&self, query: &str) -> Vec<SkillSummary> {
        let skills = self.skills.read().await;
        let query_lower = query.to_lowercase();
        let mut matches: Vec<(usize, SkillSummary)> = skills
            .values()
            .filter(|s| s.enabled)
            .filter_map(|s| {
                let name_match = s.name.to_lowercase().contains(&query_lower);
                let desc_match = s.description.to_lowercase().contains(&query_lower);
                let trigger_match = s
                    .triggers
                    .iter()
                    .any(|t| t.to_lowercase().contains(&query_lower));
                if name_match || desc_match || trigger_match {
                    let score = if name_match { 3 } else { 0 }
                        + if trigger_match { 2 } else { 0 }
                        + if desc_match { 1 } else { 0 };
                    Some((score, s.to_summary()))
                } else {
                    None
                }
            })
            .collect();
        matches.sort_by(|a, b| b.0.cmp(&a.0));
        matches.into_iter().map(|(_, s)| s).collect()
    }

    /// Search skills by query (name or description match).
    /// Returns matching skills sorted by relevance.
    pub async fn discover(&self, query: &str) -> Vec<Skill> {
        let skills = self.skills.read().await;
        let query_lower = query.to_lowercase();
        let mut matches: Vec<(usize, Skill)> = skills
            .values()
            .filter(|s| s.enabled)
            .filter_map(|s| {
                let name_match = s.name.to_lowercase().contains(&query_lower);
                let desc_match = s.description.to_lowercase().contains(&query_lower);
                let trigger_match = s.triggers.iter().any(|t| t.to_lowercase().contains(&query_lower));
                if name_match || desc_match || trigger_match {
                    let score = if name_match { 3 } else { 0 }
                        + if trigger_match { 2 } else { 0 }
                        + if desc_match { 1 } else { 0 };
                    Some((score, s.clone()))
                } else {
                    None
                }
            })
            .collect();
        matches.sort_by(|a, b| b.0.cmp(&a.0));
        matches.into_iter().map(|(_, s)| s).collect()
    }

    /// Build a plugin inventory string for the system prompt.
    /// Lists installed plugins with their env vars so the agent knows they exist.
    pub fn plugin_inventory(&self) -> String {
        let ps = match self.plugin_store {
            Some(ref ps) => ps,
            None => return String::new(),
        };
        let env_map = ps.build_env_map();
        if env_map.is_empty() {
            return String::new();
        }
        let mut lines = vec!["## Installed Plugins\n".to_string()];
        lines.push("These plugins are available via the `plugin` tool. Example: plugin(resource: \"gws\", command: \"gmail +triage --max 5\")\n".to_string());
        for (env_name, _path) in &env_map {
            let slug = env_name.trim_end_matches("_BIN").to_lowercase();
            if let Some(manifest) = ps.get_manifest(&slug) {
                lines.push(format!("- **{}** — {}", slug, manifest.description));
            } else {
                lines.push(format!("- **{}**", slug));
            }
        }
        lines.push(String::new());
        lines.join("\n")
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
        let plugins_dir = plugin_store.as_ref().map(|ps| ps.plugins_dir().to_path_buf());
        let user_plugins_dir = plugin_store.as_ref().map(|ps| ps.user_plugins_dir().to_path_buf());

        tokio::spawn(async move {
            use notify::{Event, EventKind, RecursiveMode, Watcher};
            use tokio::sync::mpsc;

            let (tx, mut rx) = mpsc::channel::<notify::Result<Event>>(32);

            let mut watcher = match notify::RecommendedWatcher::new(
                move |res| {
                    let _ = tx.blocking_send(res);
                },
                notify::Config::default()
                    .with_poll_interval(std::time::Duration::from_secs(2)),
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
                            EventKind::Create(_)
                                | EventKind::Modify(_)
                                | EventKind::Remove(_)
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
                            for mut skill in load_skills_from_nested_dir(&installed_dir, SkillSource::Installed) {
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
                                        let plugin_slug = match slug_dir.file_name().and_then(|n| n.to_str()) {
                                            Some(s) => s.to_string(),
                                            None => continue,
                                        };
                                        for mut skill in load_skills_from_nested_dir(&slug_dir, SkillSource::Installed) {
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

                        // Reload skills embedded in user plugins
                        if let Some(ref updir) = user_plugins_dir {
                            if updir.exists() {
                                if let Ok(entries) = std::fs::read_dir(updir) {
                                    for entry in entries.flatten() {
                                        let slug_dir = entry.path();
                                        if !slug_dir.is_dir() {
                                            continue;
                                        }
                                        let plugin_slug = match slug_dir.file_name().and_then(|n| n.to_str()) {
                                            Some(s) => s.to_string(),
                                            None => continue,
                                        };
                                        for mut skill in load_skills_from_nested_dir(&slug_dir, SkillSource::Installed) {
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

                        if user_dir.exists() {
                            for skill in load_skills_from_dir(&user_dir, SkillSource::User) {
                                loaded.insert(skill.name.clone(), skill);
                            }
                        }

                        verify_dependencies(&mut loaded, plugin_store.as_deref());

                        let count = loaded.len();
                        let catalog = build_catalog_string(&loaded);
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
        let ctx = super::expand::build_context(
            skill,
            self.plugin_store.as_deref(),
            store,
        );
        super::expand::expand_variables(&skill.template, &ctx)
    }
}

/// Build a names-only catalog string for the system prompt.
///
/// Like Claude Code's deferred tool listing: just skill names, comma-separated.
/// The model uses skill(action: "discover") or skill(action: "help") for details.
fn build_catalog_string(skills: &HashMap<String, Skill>) -> String {
    let mut names: Vec<&str> = skills
        .values()
        .filter(|s| s.enabled)
        .map(|s| s.name.as_str())
        .collect();
    names.sort_unstable();

    if names.is_empty() {
        return String::new();
    }

    format!(
        "## Available Skills ({} installed)\n{}\n\nUse skill(action: \"discover\", query: \"...\") to find relevant skills.\nUse skill(action: \"help\", name: \"...\") for full instructions.",
        names.len(),
        names.join(", ")
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
        format!("---\nname: {}\ndescription: {}\n---\n{}", name, name, content)
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
    let mut skills = Vec::new();
    napp::reader::walk_for_marker(dir, "SKILL.md", &mut |skill_dir| {
        let md_path = match find_skill_md(skill_dir) {
            Some(p) => p,
            None => return,
        };
        match std::fs::read(&md_path) {
            Ok(data) => match parse_skill_frontmatter(&data) {
                Ok(mut skill) => {
                    skill.enabled = true;
                    skill.source = source;
                    skill.source_path = Some(md_path);
                    skill.base_dir = Some(skill_dir.to_path_buf());
                    if skill.matches_platform() {
                        skills.push(skill);
                    } else {
                        debug!(
                            name = %skill.name,
                            platform = ?skill.platform,
                            "skipping installed skill: platform mismatch"
                        );
                    }
                }
                Err(e) => {
                    warn!(path = %skill_dir.display(), error = %e, "failed to parse SKILL.md");
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Silently skip — file may not be extracted yet
            }
            Err(e) => {
                warn!(path = %md_path.display(), error = %e, "failed to read SKILL.md");
            }
        }
    });
    skills
}

/// Load SKILL.md files from a directory (loose files).
/// Each subdirectory should contain a SKILL.md file.
fn load_skills_from_dir(dir: &Path, source: SkillSource) -> Vec<Skill> {
    let mut skills = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            warn!(error = %e, dir = %dir.display(), "failed to read skills directory");
            return skills;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            // Look for SKILL.md (case-insensitive), then SKILL.md.disabled
            let (md_path, enabled) = if let Some(p) = find_skill_md(&path) {
                (p, true)
            } else if let Some(p) = find_skill_md_disabled(&path) {
                (p, false)
            } else {
                continue;
            };

            match std::fs::read(&md_path) {
                Ok(data) => match parse_skill_frontmatter(&data) {
                    Ok(mut skill) => {
                        skill.enabled = enabled;
                        skill.source_path = Some(md_path);
                        skill.source = source;
                        skill.base_dir = Some(path.clone());
                        if skill.matches_platform() {
                            skills.push(skill);
                        } else {
                            debug!(
                                name = %skill.name,
                                platform = ?skill.platform,
                                "skipping skill: platform mismatch"
                            );
                        }
                    }
                    Err(e) => {
                        warn!(path = %md_path.display(), error = %e, "failed to parse SKILL.md");
                    }
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Silently skip — file may not be extracted yet
                }
                Err(e) => {
                    warn!(path = %md_path.display(), error = %e, "failed to read SKILL.md");
                }
            }
        }
    }

    skills
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
        assert!(count >= 1, "should load at least the user skill (plus bundled)");

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
        assert!(count >= 1, "should load at least the installed skill (plus bundled)");

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

    /// Create a skill inside a plugin's skills/ subdirectory (simulates embedded plugin skill).
    fn create_plugin_embedded_skill(plugins_dir: &Path, slug: &str, version: &str, skill_name: &str, skill_md: &str) {
        let skill_dir = plugins_dir.join(slug).join(version).join("skills").join(skill_name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), skill_md).unwrap();
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
            &BASIC_SKILL.replace("test-skill", "outreach-email").replace("A test skill", "Send outreach emails"),
        );

        let loader = Loader::new(installed.path().to_path_buf(), user.path().to_path_buf())
            .with_plugin_store(plugin_store);
        let count = loader.load_all().await;
        assert!(count >= 1, "should load at least the plugin skill (plus bundled)");

        let skill = loader.get("outreach-email").await.unwrap();
        assert_eq!(skill.description, "Send outreach emails");
        assert!(skill.enabled);
        // Should auto-inject the parent plugin as a dependency
        assert!(skill.plugins.iter().any(|p| p.name == "outreach"), "should have outreach plugin dependency");
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
            &BASIC_SKILL.replace("test-skill", "gws-gmail").replace("A test skill", "Marketplace version"),
        );
        create_plugin_embedded_skill(
            user_plugins.path(),
            "gws",
            "1.0.0",
            "gws-gmail",
            &BASIC_SKILL.replace("test-skill", "gws-gmail").replace("A test skill", "User version"),
        );

        let loader = Loader::new(installed.path().to_path_buf(), user.path().to_path_buf())
            .with_plugin_store(plugin_store);
        loader.load_all().await;

        let skill = loader.get("gws-gmail").await.unwrap();
        assert_eq!(skill.description, "User version", "user plugin skills should override marketplace plugin skills");
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
