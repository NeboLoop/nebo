use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::skill::{parse_skill_md, Skill, SkillSource};

/// Manages loading, caching, and hot-reloading of skills from bundled,
/// sealed .napp archives (nebo/skills/) and loose files (user/skills/).
pub struct Loader {
    /// Bundled skills directory (shipped with app, e.g. <data_dir>/bundled/skills/).
    bundled_dir: PathBuf,
    /// User skills directory (e.g. <data_dir>/user/skills/).
    user_dir: PathBuf,
    /// Installed (marketplace) skills directory (e.g. <data_dir>/nebo/skills/).
    installed_dir: PathBuf,
    /// Loaded skills keyed by name.
    skills: Arc<RwLock<HashMap<String, Skill>>>,
    /// Optional plugin store for verifying plugin dependencies.
    plugin_store: Option<Arc<napp::plugin::PluginStore>>,
}

impl Loader {
    pub fn new(bundled_dir: PathBuf, installed_dir: PathBuf, user_dir: PathBuf) -> Self {
        Self {
            bundled_dir,
            user_dir,
            installed_dir,
            skills: Arc::new(RwLock::new(HashMap::new())),
            plugin_store: None,
        }
    }

    /// Set the plugin store for verifying plugin dependencies during load.
    pub fn with_plugin_store(mut self, store: Arc<napp::plugin::PluginStore>) -> Self {
        self.plugin_store = Some(store);
        self
    }

    /// Load all skills from bundled, installed (.napp) and user (loose files) directories.
    /// Loading order: bundled → installed (override by name) → user (override by name).
    /// After loading, verifies dependencies — skills with missing deps are dropped.
    pub async fn load_all(&self) -> usize {
        let mut loaded = HashMap::new();

        // 1. Load bundled skills (shipped with app)
        if self.bundled_dir.exists() {
            for mut skill in load_skills_from_dir(&self.bundled_dir, SkillSource::Installed) {
                skill.enabled = true;
                loaded.insert(skill.name.clone(), skill);
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

        // 3. Load user skills (override installed by name)
        if self.user_dir.exists() {
            for skill in load_skills_from_dir(&self.user_dir, SkillSource::User) {
                loaded.insert(skill.name.clone(), skill);
            }
        }

        // Verify dependencies — skip skills with missing deps or required plugins
        verify_dependencies(&mut loaded, self.plugin_store.as_deref());

        let count = loaded.len();
        *self.skills.write().await = loaded;
        info!(count, installed_dir = %self.installed_dir.display(), user_dir = %self.user_dir.display(), "loaded skills");
        count
    }

    /// Get a skill by name.
    pub async fn get(&self, name: &str) -> Option<Skill> {
        self.skills.read().await.get(name).cloned()
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
    /// Get the bundled skills directory path.
    pub fn bundled_dir(&self) -> &Path {
        &self.bundled_dir
    }

    pub fn watch(&self) -> tokio::task::JoinHandle<()> {
        let bundled_dir = self.bundled_dir.clone();
        let user_dir = self.user_dir.clone();
        let installed_dir = self.installed_dir.clone();
        let skills = self.skills.clone();
        let plugin_store = self.plugin_store.clone();
        let plugins_dir = plugin_store.as_ref().map(|ps| ps.plugins_dir().to_path_buf());

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

                        if last_reload.elapsed() < debounce {
                            continue;
                        }
                        last_reload = std::time::Instant::now();

                        debug!("skills directory changed, reloading");
                        let mut loaded = HashMap::new();

                        if bundled_dir.exists() {
                            for mut skill in load_skills_from_dir(&bundled_dir, SkillSource::Installed) {
                                skill.enabled = true;
                                loaded.insert(skill.name.clone(), skill);
                            }
                        }

                        if installed_dir.exists() {
                            for mut skill in load_skills_from_nested_dir(&installed_dir, SkillSource::Installed) {
                                skill.enabled = true;
                                loaded.insert(skill.name.clone(), skill);
                            }
                        }

                        // Reload skills embedded in plugins
                        if let Some(ref pdir) = plugins_dir {
                            if pdir.exists() {
                                for mut skill in load_skills_from_nested_dir(pdir, SkillSource::Installed) {
                                    skill.enabled = true;
                                    loaded.insert(skill.name.clone(), skill);
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
            Ok(data) => match parse_skill_md(&data) {
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
                Ok(data) => match parse_skill_md(&data) {
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

/// Verify skill dependencies — drop skills with missing deps or required plugins.
fn verify_dependencies(
    loaded: &mut HashMap<String, Skill>,
    plugin_store: Option<&napp::plugin::PluginStore>,
) {
    let names: HashSet<String> = loaded.keys().cloned().collect();
    loaded.retain(|name, skill| {
        // Check inter-skill dependencies
        for dep in &skill.dependencies {
            if !names.contains(dep) {
                warn!(skill = %name, missing_dep = %dep, "skill skipped: missing dependency");
                return false;
            }
        }
        // Check plugin dependencies (only required ones)
        if let Some(store) = plugin_store {
            for p in &skill.plugins {
                if p.optional {
                    continue;
                }
                if store.resolve(&p.name, &p.version).is_none() {
                    warn!(skill = %name, plugin = %p.name, version = %p.version, "skill skipped: missing required plugin");
                    return false;
                }
            }
        }
        true
    });
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

        let bundled = TempDir::new().unwrap();
        let loader = Loader::new(bundled.path().to_path_buf(), installed.path().to_path_buf(), tmp.path().to_path_buf());
        let count = loader.load_all().await;
        assert_eq!(count, 1);

        let skill = loader.get("test-skill").await.unwrap();
        assert_eq!(skill.description, "A test skill");
        assert_eq!(skill.priority, 5);
        assert!(skill.template.contains("test skill template"));
        assert_eq!(skill.source, SkillSource::User);
    }

    #[tokio::test]
    async fn test_load_from_extracted() {
        let bundled = TempDir::new().unwrap();
        let installed = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();

        create_skill_extracted(
            installed.path(),
            "@acme/skills/test",
            "1.0.0",
            BASIC_SKILL.as_bytes(),
        );

        let loader = Loader::new(bundled.path().to_path_buf(), installed.path().to_path_buf(), user.path().to_path_buf());
        let count = loader.load_all().await;
        assert_eq!(count, 1);

        let skill = loader.get("test-skill").await.unwrap();
        assert_eq!(skill.source, SkillSource::Installed);
        assert!(skill.base_dir.is_some());
    }

    #[tokio::test]
    async fn test_user_overrides_installed() {
        let bundled = TempDir::new().unwrap();
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

        let loader = Loader::new(bundled.path().to_path_buf(), installed.path().to_path_buf(), user.path().to_path_buf());
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

        let bundled = TempDir::new().unwrap();
        let loader = Loader::new(bundled.path().to_path_buf(), installed.path().to_path_buf(), tmp.path().to_path_buf());
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

        let bundled = TempDir::new().unwrap();
        let loader = Loader::new(bundled.path().to_path_buf(), installed.path().to_path_buf(), tmp.path().to_path_buf());
        loader.load_all().await;

        let matches = loader.match_triggers("please test trigger this", 3).await;
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "test-skill");

        let no_match = loader.match_triggers("unrelated message", 3).await;
        assert!(no_match.is_empty());
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

        let bundled = TempDir::new().unwrap();
        let loader = Loader::new(bundled.path().to_path_buf(), installed.path().to_path_buf(), tmp.path().to_path_buf());
        loader.load_all().await;

        let list = loader.list().await;
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "high");
        assert_eq!(list[1].name, "low");
    }

}
