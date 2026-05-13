//! Skill manifest index for fast warm-start loading.
//!
//! On cold start, `load_all()` walks the filesystem and parses every SKILL.md.
//! On completion, it writes a `.skill-manifest.json` file with skill metadata
//! and content hashes. On warm start, we read this single file instead of
//! walking the filesystem — turning ~20s into <50ms.
//!
//! The manifest stores enough to reconstruct a full `Skill` (minus template,
//! which is lazy-loaded on first `get()` call).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use super::skill::{PluginDependency, Skill, SkillRequirement, SkillSource};

/// Current manifest format version. Bump when the schema changes.
const MANIFEST_VERSION: u32 = 1;

/// A persisted skill manifest for fast warm-start loading.
#[derive(Debug, Serialize, Deserialize)]
pub struct SkillManifest {
    pub version: u32,
    pub skills: HashMap<String, SkillEntry>,
}

/// One skill's metadata in the manifest. Stores all `Skill` fields that are
/// marked `#[serde(skip)]` (and thus don't round-trip through the Skill struct's
/// own Serialize/Deserialize), plus a content hash for staleness detection.
#[derive(Debug, Serialize, Deserialize)]
pub struct SkillEntry {
    /// xxh3 hash of the SKILL.md file bytes (or 0 for bundled skills).
    pub content_hash: u64,

    // ── Fields from Skill that round-trip via serde ─────────────────
    pub name: String,
    pub description: String,
    pub license: String,
    pub compatibility: String,
    pub allowed_tools: String,
    pub metadata: HashMap<String, serde_json::Value>,
    pub version: String,
    pub author: String,
    pub dependencies: Vec<String>,
    pub tags: Vec<String>,
    pub platform: Vec<String>,
    pub triggers: Vec<String>,
    pub capabilities: Vec<String>,
    pub priority: i32,
    pub max_turns: i32,
    pub plugins: Vec<PluginDependency>,
    pub requires: Vec<SkillRequirement>,
    pub source: SkillSource,

    // ── Fields marked #[serde(skip)] on Skill — stored explicitly ──
    pub enabled: bool,
    pub degraded: Option<String>,
    pub source_path: Option<PathBuf>,
    pub base_dir: Option<PathBuf>,
    pub napp_path: Option<PathBuf>,
    // license_key is intentionally excluded — it's a runtime secret
    // loaded from the license key cache, never persisted in the manifest.
}

impl SkillManifest {
    /// Load a manifest from disk. Returns Err if the file doesn't exist,
    /// is corrupt, or has a different version.
    pub fn load(path: &Path) -> Result<Self, String> {
        let data =
            std::fs::read(path).map_err(|e| format!("failed to read manifest: {}", e))?;
        let manifest: Self =
            serde_json::from_slice(&data).map_err(|e| format!("failed to parse manifest: {}", e))?;
        if manifest.version != MANIFEST_VERSION {
            return Err(format!(
                "manifest version mismatch: expected {}, got {}",
                MANIFEST_VERSION, manifest.version
            ));
        }
        Ok(manifest)
    }

    /// Write the manifest to disk atomically (write to .tmp, then rename).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let tmp = path.with_extension("tmp");
        let data =
            serde_json::to_vec(self).map_err(|e| format!("failed to serialize manifest: {}", e))?;
        std::fs::write(&tmp, &data)
            .map_err(|e| format!("failed to write manifest tmp: {}", e))?;
        std::fs::rename(&tmp, path)
            .map_err(|e| format!("failed to rename manifest: {}", e))?;
        debug!(path = %path.display(), skills = self.skills.len(), "wrote skill manifest");
        Ok(())
    }

    /// Convert manifest entries into a HashMap<String, Skill> for the loader.
    pub fn into_skill_map(self) -> HashMap<String, Skill> {
        self.skills
            .into_iter()
            .map(|(name, entry)| {
                let skill = Skill {
                    name: entry.name,
                    description: entry.description,
                    license: entry.license,
                    compatibility: entry.compatibility,
                    allowed_tools: entry.allowed_tools,
                    metadata: entry.metadata,
                    version: entry.version,
                    author: entry.author,
                    dependencies: entry.dependencies,
                    tags: entry.tags,
                    platform: entry.platform,
                    triggers: entry.triggers,
                    capabilities: entry.capabilities,
                    priority: entry.priority,
                    max_turns: entry.max_turns,
                    plugins: entry.plugins,
                    requires: entry.requires,
                    source: entry.source,
                    // #[serde(skip)] fields restored from manifest
                    enabled: entry.enabled,
                    degraded: entry.degraded,
                    source_path: entry.source_path,
                    base_dir: entry.base_dir,
                    napp_path: entry.napp_path,
                    // Runtime-only fields
                    template: String::new(), // lazy-loaded on get()
                    license_key: None,       // loaded from license key cache at runtime
                };
                (name, skill)
            })
            .collect()
    }

    /// Build a manifest from a loaded skill map.
    pub fn from_skill_map(skills: &HashMap<String, Skill>, hashes: &HashMap<String, u64>) -> Self {
        let entries = skills
            .iter()
            .map(|(name, skill)| {
                let entry = SkillEntry {
                    content_hash: hashes.get(name).copied().unwrap_or(0),
                    name: skill.name.clone(),
                    description: skill.description.clone(),
                    license: skill.license.clone(),
                    compatibility: skill.compatibility.clone(),
                    allowed_tools: skill.allowed_tools.clone(),
                    metadata: skill.metadata.clone(),
                    version: skill.version.clone(),
                    author: skill.author.clone(),
                    dependencies: skill.dependencies.clone(),
                    tags: skill.tags.clone(),
                    platform: skill.platform.clone(),
                    triggers: skill.triggers.clone(),
                    capabilities: skill.capabilities.clone(),
                    priority: skill.priority,
                    max_turns: skill.max_turns,
                    plugins: skill.plugins.clone(),
                    requires: skill.requires.clone(),
                    source: skill.source,
                    enabled: skill.enabled,
                    degraded: skill.degraded.clone(),
                    source_path: skill.source_path.clone(),
                    base_dir: skill.base_dir.clone(),
                    napp_path: skill.napp_path.clone(),
                };
                (name.clone(), entry)
            })
            .collect();
        Self {
            version: MANIFEST_VERSION,
            skills: entries,
        }
    }
}

/// Compute xxh3 hash of a file's contents.
pub fn hash_file(path: &Path) -> Option<u64> {
    let data = std::fs::read(path).ok()?;
    Some(xxhash_rust::xxh3::xxh3_64(&data))
}

/// Compute content hashes for all skills that have a source_path.
pub fn compute_hashes(skills: &HashMap<String, Skill>) -> HashMap<String, u64> {
    skills
        .iter()
        .filter_map(|(name, skill)| {
            let path = skill.source_path.as_ref()?;
            let hash = hash_file(path)?;
            Some((name.clone(), hash))
        })
        .collect()
}

/// Verify a manifest against the current filesystem state.
/// Returns (stale_names, new_paths) where:
/// - stale_names: skills whose source file hash has changed or been removed
/// - new_paths: SKILL.md paths found on disk but not in the manifest
pub fn verify_manifest(
    manifest: &SkillManifest,
    installed_dir: &Path,
    user_dir: &Path,
    plugins_dirs: &[PathBuf],
) -> (Vec<String>, Vec<PathBuf>) {
    let mut stale = Vec::new();

    // Check existing entries for hash changes / removals
    for (name, entry) in &manifest.skills {
        if let Some(ref path) = entry.source_path {
            match hash_file(path) {
                Some(hash) if hash == entry.content_hash => {} // unchanged
                Some(_) => stale.push(name.clone()),           // content changed
                None => stale.push(name.clone()),              // file removed
            }
        }
        // Bundled skills (no source_path) are always fresh — compiled in
    }

    // Find new SKILL.md files not in the manifest
    let known_paths: std::collections::HashSet<PathBuf> = manifest
        .skills
        .values()
        .filter_map(|e| e.source_path.clone())
        .collect();
    // Also track by name: directory name == skill name (Agent Skills spec).
    // This prevents marketplace copies of user-overridden skills from being
    // "discovered" as new when the manifest only stores the winning path.
    let known_names: std::collections::HashSet<&str> = manifest
        .skills
        .keys()
        .map(|s| s.as_str())
        .collect();
    let mut new_paths = Vec::new();

    let mut check_dir = |dir: &Path| {
        napp::reader::walk_for_marker(dir, "SKILL.md", &mut |skill_dir| {
            let md_path = skill_dir.join("SKILL.md");
            if known_paths.contains(&md_path) {
                return; // exact path match — already tracked
            }
            // Check by name: directory name is the skill name per spec
            let dir_name = skill_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if known_names.contains(dir_name) {
                return; // different path but same skill name — overridden copy
            }
            new_paths.push(md_path);
        });
    };

    if installed_dir.exists() {
        check_dir(installed_dir);
    }
    if user_dir.exists() {
        check_dir(user_dir);
    }
    for pdir in plugins_dirs {
        if pdir.exists() {
            check_dir(pdir);
        }
    }

    info!(
        stale = stale.len(),
        new = new_paths.len(),
        "manifest verification complete"
    );
    (stale, new_paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_round_trip() {
        let mut skills = HashMap::new();
        let skill = Skill {
            name: "test-skill".into(),
            description: "A test skill".into(),
            version: "1.0.0".into(),
            license: String::new(),
            compatibility: String::new(),
            allowed_tools: String::new(),
            author: String::new(),
            dependencies: vec![],
            tags: vec!["test".into()],
            platform: vec![],
            triggers: vec!["test".into()],
            capabilities: vec![],
            priority: 10,
            max_turns: 0,
            plugins: vec![],
            requires: vec![],
            metadata: HashMap::new(),
            template: "should not be serialized".into(),
            enabled: true,
            degraded: None,
            source_path: Some(PathBuf::from("/tmp/test/SKILL.md")),
            source: SkillSource::User,
            base_dir: Some(PathBuf::from("/tmp/test")),
            napp_path: None,
            license_key: None,
        };
        skills.insert("test-skill".into(), skill);

        let hashes = {
            let mut h = HashMap::new();
            h.insert("test-skill".into(), 12345u64);
            h
        };

        let manifest = SkillManifest::from_skill_map(&skills, &hashes);
        assert_eq!(manifest.version, MANIFEST_VERSION);
        assert_eq!(manifest.skills.len(), 1);

        let entry = &manifest.skills["test-skill"];
        assert_eq!(entry.content_hash, 12345);
        assert_eq!(entry.name, "test-skill");
        assert!(entry.enabled);
        assert_eq!(entry.tags, vec!["test"]);

        // Round-trip back to Skill
        let restored = manifest.into_skill_map();
        let s = &restored["test-skill"];
        assert_eq!(s.name, "test-skill");
        assert_eq!(s.priority, 10);
        assert!(s.enabled);
        assert!(s.template.is_empty()); // template is NOT preserved
        assert!(s.license_key.is_none()); // license_key is NOT preserved
        assert_eq!(
            s.source_path,
            Some(PathBuf::from("/tmp/test/SKILL.md"))
        );
    }

    #[test]
    fn test_manifest_save_load() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join(".skill-manifest.json");

        let manifest = SkillManifest {
            version: MANIFEST_VERSION,
            skills: HashMap::new(),
        };
        manifest.save(&path).unwrap();

        let loaded = SkillManifest::load(&path).unwrap();
        assert_eq!(loaded.version, MANIFEST_VERSION);
        assert!(loaded.skills.is_empty());
    }

    #[test]
    fn test_manifest_version_mismatch() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join(".skill-manifest.json");

        let manifest = SkillManifest {
            version: 999,
            skills: HashMap::new(),
        };
        let data = serde_json::to_vec(&manifest).unwrap();
        std::fs::write(&path, &data).unwrap();

        let result = SkillManifest::load(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("version mismatch"));
    }
}
