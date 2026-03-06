use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::skill::{parse_skill_md, Skill};

/// Manages loading, caching, and hot-reloading of SKILL.md files.
pub struct Loader {
    /// User skills directory (e.g. <data_dir>/skills/).
    skills_dir: PathBuf,
    /// Optional bundled skills directory (shipped with the app).
    bundled_dir: Option<PathBuf>,
    /// Loaded skills keyed by name.
    skills: Arc<RwLock<HashMap<String, Skill>>>,
}

impl Loader {
    pub fn new(skills_dir: PathBuf, bundled_dir: Option<PathBuf>) -> Self {
        Self {
            skills_dir,
            bundled_dir,
            skills: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Load all skills from bundled and user directories.
    /// Bundled skills are loaded first; user skills override by name.
    pub async fn load_all(&self) -> usize {
        let mut loaded = HashMap::new();

        // Load bundled skills first
        if let Some(ref bundled) = self.bundled_dir {
            if bundled.exists() {
                for mut skill in load_skills_from_dir(bundled) {
                    skill.enabled = true;
                    loaded.insert(skill.name.clone(), skill);
                }
            }
        }

        // Load user skills (override bundled by name)
        if self.skills_dir.exists() {
            for skill in load_skills_from_dir(&self.skills_dir) {
                loaded.insert(skill.name.clone(), skill);
            }
        }

        // Also load flat .yaml / .yaml.disabled files for backward compatibility
        for skill in load_yaml_skills(&self.skills_dir) {
            if !loaded.contains_key(&skill.name) {
                loaded.insert(skill.name.clone(), skill);
            }
        }

        let count = loaded.len();
        *self.skills.write().await = loaded;
        info!(count, dir = %self.skills_dir.display(), "loaded skills");
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

    /// Start watching for filesystem changes and reload on modification.
    /// Returns a JoinHandle that runs until cancelled.
    pub fn watch(&self) -> tokio::task::JoinHandle<()> {
        let skills_dir = self.skills_dir.clone();
        let bundled_dir = self.bundled_dir.clone();
        let skills = self.skills.clone();

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

            if skills_dir.exists() {
                if let Err(e) = watcher.watch(&skills_dir, RecursiveMode::Recursive) {
                    warn!(error = %e, dir = %skills_dir.display(), "failed to watch skills dir");
                }
            }

            if let Some(ref bundled) = bundled_dir {
                if bundled.exists() {
                    if let Err(e) = watcher.watch(bundled, RecursiveMode::Recursive) {
                        warn!(error = %e, dir = %bundled.display(), "failed to watch bundled skills dir");
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
                                || name.ends_with(".yaml")
                                || name.ends_with(".yaml.disabled")
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

                        if let Some(ref bundled) = bundled_dir {
                            if bundled.exists() {
                                for mut skill in load_skills_from_dir(bundled) {
                                    skill.enabled = true;
                                    loaded.insert(skill.name.clone(), skill);
                                }
                            }
                        }

                        if skills_dir.exists() {
                            for skill in load_skills_from_dir(&skills_dir) {
                                loaded.insert(skill.name.clone(), skill);
                            }
                        }

                        for skill in load_yaml_skills(&skills_dir) {
                            if !loaded.contains_key(&skill.name) {
                                loaded.insert(skill.name.clone(), skill);
                            }
                        }

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
    pub fn skills_dir(&self) -> &Path {
        &self.skills_dir
    }
}

/// Load SKILL.md files from a directory.
/// Each subdirectory should contain a SKILL.md file.
fn load_skills_from_dir(dir: &Path) -> Vec<Skill> {
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
            // Look for SKILL.md (case-insensitive) inside the subdirectory
            if let Some(md_path) = find_skill_md(&path) {
                match std::fs::read(&md_path) {
                    Ok(data) => match parse_skill_md(&data) {
                        Ok(mut skill) => {
                            skill.enabled = true;
                            skill.source_path = Some(md_path);
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

/// Load flat .yaml / .yaml.disabled skill files for backward compatibility.
fn load_yaml_skills(dir: &Path) -> Vec<Skill> {
    let mut skills = Vec::new();
    if !dir.exists() {
        return skills;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return skills,
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let (slug, enabled) = if name.ends_with(".yaml.disabled") {
            (name.trim_end_matches(".yaml.disabled").to_string(), false)
        } else if name.ends_with(".yaml") {
            (name.trim_end_matches(".yaml").to_string(), true)
        } else {
            continue;
        };

        if let Ok(content) = std::fs::read_to_string(entry.path()) {
            skills.push(Skill {
                name: slug,
                description: "YAML skill (legacy format)".to_string(),
                version: "1.0.0".into(),
                author: String::new(),
                dependencies: vec![],
                tags: vec![],
                platform: vec![],
                triggers: vec![],
                tools: vec![],
                priority: 0,
                max_turns: 0,
                metadata: HashMap::new(),
                template: content,
                enabled,
                source_path: Some(entry.path()),
            });
        }
    }

    skills
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
        create_skill_md(tmp.path(), "test-skill", BASIC_SKILL);

        let loader = Loader::new(tmp.path().to_path_buf(), None);
        let count = loader.load_all().await;
        assert_eq!(count, 1);

        let skill = loader.get("test-skill").await.unwrap();
        assert_eq!(skill.description, "A test skill");
        assert_eq!(skill.priority, 5);
        assert!(skill.template.contains("test skill template"));
    }

    #[tokio::test]
    async fn test_platform_filtering() {
        let tmp = TempDir::new().unwrap();
        create_skill_md(tmp.path(), "windows-only", PLATFORM_SKILL);
        create_skill_md(tmp.path(), "test-skill", BASIC_SKILL);

        let loader = Loader::new(tmp.path().to_path_buf(), None);
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
        let tmp = TempDir::new().unwrap();
        create_skill_md(tmp.path(), "test-skill", BASIC_SKILL);

        let loader = Loader::new(tmp.path().to_path_buf(), None);
        loader.load_all().await;

        let matches = loader.match_triggers("please test trigger this", 3).await;
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "test-skill");

        let no_match = loader.match_triggers("unrelated message", 3).await;
        assert!(no_match.is_empty());
    }

    #[tokio::test]
    async fn test_list_sorted_by_priority() {
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

        let loader = Loader::new(tmp.path().to_path_buf(), None);
        loader.load_all().await;

        let list = loader.list().await;
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "high");
        assert_eq!(list[1].name, "low");
    }

    #[tokio::test]
    async fn test_bundled_override() {
        let bundled = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();

        create_skill_md(bundled.path(), "shared", BASIC_SKILL);
        create_skill_md(
            user.path(),
            "shared",
            &BASIC_SKILL.replace("A test skill", "User override"),
        );

        let loader = Loader::new(user.path().to_path_buf(), Some(bundled.path().to_path_buf()));
        loader.load_all().await;

        let skill = loader.get("test-skill").await.unwrap();
        assert_eq!(skill.description, "User override");
    }

    #[tokio::test]
    async fn test_yaml_backward_compat() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("legacy.yaml"),
            "name: legacy\ncontent: old format",
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("disabled.yaml.disabled"),
            "name: disabled",
        )
        .unwrap();

        let loader = Loader::new(tmp.path().to_path_buf(), None);
        loader.load_all().await;

        let legacy = loader.get("legacy").await.unwrap();
        assert!(legacy.enabled);
        assert!(legacy.template.contains("old format"));

        let disabled = loader.get("disabled").await.unwrap();
        assert!(!disabled.enabled);
    }
}
