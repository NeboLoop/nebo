use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A secret declared in a skill's metadata.secrets array.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretDeclaration {
    /// Environment variable name (e.g., "BRAVE_API_KEY").
    pub key: String,
    /// Human-readable label for the UI.
    pub label: String,
    /// Help text (e.g., URL to get the key).
    pub hint: String,
    /// Whether the skill requires this secret to function.
    pub required: bool,
}

/// A plugin dependency declared in a skill's frontmatter.
///
/// ```yaml
/// plugins:
///   - name: gws
///     version: ">=1.2.0"
///   - name: ffmpeg
///     version: ">=5.0.0"
///     optional: true
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDependency {
    /// Plugin slug (matches the plugin's registered name in NeboLoop).
    pub name: String,
    /// Semver version range. Defaults to `"*"` (any version).
    #[serde(default = "default_version_range")]
    pub version: String,
    /// If true, the skill loads even without this plugin installed.
    #[serde(default)]
    pub optional: bool,
}

/// A skill-to-skill dependency declared in a skill's frontmatter.
///
/// ```yaml
/// requires:
///   - name: web-search
///     version: ">=1.0.0"
///   - name: data-analysis
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRequirement {
    /// Skill name (must match the loaded skill's `name` field).
    pub name: String,
    /// Semver version range. Defaults to `"*"` (any version).
    #[serde(default = "default_version_range")]
    pub version: String,
}

fn default_version_range() -> String {
    "*".to_string()
}

/// Where a skill was loaded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    /// Installed from NeboLoop marketplace (sealed .napp archive).
    Installed,
    /// User-created (loose files in user/ directory).
    User,
}

/// Lightweight view of a skill for list/search/catalog operations.
///
/// Avoids cloning heavy fields (metadata HashMap, plugins Vec, requires Vec, etc.)
/// that list/discover consumers never read.
#[derive(Debug, Clone, Serialize)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub version: String,
    pub enabled: bool,
    pub source: SkillSource,
    pub triggers: Vec<String>,
    pub capabilities: Vec<String>,
    pub source_path: Option<PathBuf>,
    pub base_dir: Option<PathBuf>,
    pub priority: i32,
    /// True if the skill declares secrets in metadata.
    pub has_secrets: bool,
    pub degraded: Option<String>,
}

/// A skill parsed from a SKILL.md file with YAML frontmatter.
///
/// Implements the Agent Skills standard (https://skill.md) plus Nebo extensions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    // ── Agent Skills Standard Fields ────────────────────────────────
    pub name: String,
    pub description: String,
    /// License name or reference to bundled license file.
    #[serde(default)]
    pub license: String,
    /// Environment requirements (intended product, system packages, network, etc.).
    #[serde(default)]
    pub compatibility: String,
    /// Pre-approved tools the skill may use (space-delimited). Experimental.
    #[serde(default, alias = "allowed-tools")]
    pub allowed_tools: String,
    /// Arbitrary key-value metadata.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,

    // ── Nebo Extension Fields ───────────────────────────────────────
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub platform: Vec<String>,
    #[serde(default)]
    pub triggers: Vec<String>,
    /// Platform capabilities this skill needs.
    /// e.g., ["python", "storage", "vision"]
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub max_turns: i32,
    /// Shared plugin binaries this skill depends on.
    #[serde(default)]
    pub plugins: Vec<PluginDependency>,
    /// Skill-to-skill dependencies with optional version ranges.
    #[serde(default)]
    pub requires: Vec<SkillRequirement>,
    /// The markdown body (not from YAML — parsed from the content after frontmatter).
    #[serde(skip)]
    pub template: String,
    /// Whether this skill is currently enabled.
    #[serde(skip)]
    pub enabled: bool,
    /// If set, this skill has unmet dependencies and is degraded.
    /// The string describes which dependencies are missing or version-incompatible.
    #[serde(skip)]
    pub degraded: Option<String>,
    /// Filesystem path this skill was loaded from.
    #[serde(skip)]
    pub source_path: Option<PathBuf>,
    /// Where this skill was loaded from (marketplace vs user).
    #[serde(default = "default_source")]
    pub source: SkillSource,
    /// Root directory of the skill (parent of SKILL.md).
    #[serde(skip)]
    pub base_dir: Option<PathBuf>,
    /// Path to the sealed .napp archive (for paid content read in memory).
    #[serde(skip)]
    pub napp_path: Option<PathBuf>,
    /// License key for reading from sealed .napp (kept in memory only).
    #[serde(skip)]
    pub license_key: Option<[u8; 32]>,
}

fn default_source() -> SkillSource {
    SkillSource::User
}

fn default_version() -> String {
    "1.0.0".to_string()
}

impl Skill {
    /// Create a lightweight summary for list/search operations.
    pub fn to_summary(&self) -> SkillSummary {
        SkillSummary {
            name: self.name.clone(),
            description: self.description.clone(),
            version: self.version.clone(),
            enabled: self.enabled,
            source: self.source,
            triggers: self.triggers.clone(),
            capabilities: self.capabilities.clone(),
            source_path: self.source_path.clone(),
            base_dir: self.base_dir.clone(),
            priority: self.priority,
            has_secrets: self.metadata.contains_key("secrets"),
            degraded: self.degraded.clone(),
        }
    }

    /// Validate that required fields are present and conform to the Agent Skills standard.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("skill name is required".into());
        }
        if self.name.len() > 64 {
            return Err(format!("skill name exceeds 64 characters: {}", self.name.len()));
        }
        if self.name.starts_with('-') || self.name.ends_with('-') {
            return Err("skill name must not start or end with a hyphen".into());
        }
        if self.name.contains("--") {
            return Err("skill name must not contain consecutive hyphens".into());
        }
        if !self.name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err("skill name must contain only lowercase letters, digits, and hyphens".into());
        }
        if self.description.is_empty() {
            return Err("skill description is required".into());
        }
        if self.description.len() > 1024 {
            return Err(format!("skill description exceeds 1024 characters: {}", self.description.len()));
        }
        if self.compatibility.len() > 500 {
            return Err(format!("compatibility exceeds 500 characters: {}", self.compatibility.len()));
        }
        Ok(())
    }

    /// Check if this skill matches the current platform.
    pub fn matches_platform(&self) -> bool {
        if self.platform.is_empty() {
            return true;
        }
        let current = current_platform();
        self.platform
            .iter()
            .any(|p| p.eq_ignore_ascii_case(&current))
    }

    /// Check if this skill requires cloud sandbox execution.
    pub fn needs_sandbox(&self) -> bool {
        self.capabilities
            .iter()
            .any(|c| matches!(c.as_str(), "python" | "typescript"))
    }

    /// Extract secret declarations from metadata.secrets.
    ///
    /// Skills declare required secrets in SKILL.md frontmatter:
    /// ```yaml
    /// metadata:
    ///   secrets:
    ///     - key: BRAVE_API_KEY
    ///       label: "Brave Search API Key"
    ///       hint: "https://brave.com/search/api/"
    ///       required: true
    /// ```
    pub fn secrets(&self) -> Vec<SecretDeclaration> {
        let Some(secrets_val) = self.metadata.get("secrets") else {
            return vec![];
        };
        let Some(arr) = secrets_val.as_array() else {
            return vec![];
        };
        arr.iter()
            .filter_map(|v| {
                let key = v.get("key")?.as_str()?.to_string();
                Some(SecretDeclaration {
                    key,
                    label: v.get("label").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    hint: v.get("hint").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    required: v.get("required").and_then(|v| v.as_bool()).unwrap_or(false),
                })
            })
            .collect()
    }

    /// List resource files (not SKILL.md itself).
    ///
    /// For sealed skills, lists entries from the encrypted .napp archive in memory,
    /// filtering out SKILL.md and metadata files. For free skills, walks the
    /// extracted directory on disk.
    pub fn list_resources(&self) -> Result<Vec<String>, String> {
        // Sealed .napp: list from archive in memory
        if let (Some(napp_path), Some(key)) = (&self.napp_path, &self.license_key) {
            let entries = napp::reader::list_sealed_napp_entries(napp_path, key)
                .map_err(|e| format!("failed to list sealed resources: {}", e))?;
            return Ok(entries
                .into_iter()
                .filter(|name| !is_metadata_entry(name))
                .collect());
        }

        // Free content: walk extracted directory
        if let Some(ref base_dir) = self.base_dir {
            let mut resources = Vec::new();
            walk_resources(base_dir, base_dir, &mut resources);
            Ok(resources)
        } else {
            Ok(vec![])
        }
    }

    /// Read a resource file by relative path.
    /// Path traversal (`..`) is rejected.
    ///
    /// For sealed skills, reads from the encrypted .napp archive in memory
    /// (plaintext never touches disk). For free skills, reads from the
    /// extracted directory on disk.
    pub fn read_resource(&self, relative_path: &str) -> Result<Vec<u8>, String> {
        if relative_path.contains("..") {
            return Err("path traversal not allowed".into());
        }

        // Sealed .napp: read from archive in memory
        if let (Some(napp_path), Some(key)) = (&self.napp_path, &self.license_key) {
            return napp::reader::read_sealed_napp_entry(napp_path, relative_path, key)
                .map_err(|e| format!("failed to read sealed resource: {}", e));
        }

        // Free content: read from extracted directory
        if let Some(ref base_dir) = self.base_dir {
            let full = base_dir.join(relative_path);
            // Guard against symlink escapes
            if !full.starts_with(base_dir) {
                return Err("path traversal not allowed".into());
            }
            std::fs::read(&full).map_err(|e| format!("failed to read resource: {}", e))
        } else {
            Err("skill has no resource directory".into())
        }
    }

    /// Whether this skill's content is sealed (paid, encrypted at rest).
    pub fn is_sealed(&self) -> bool {
        self.napp_path.is_some() && self.license_key.is_some()
    }

    /// Check if a message matches any of this skill's triggers (case-insensitive substring).
    pub fn matches_trigger(&self, message: &str) -> bool {
        if self.triggers.is_empty() {
            return false;
        }
        let msg_lower = message.to_lowercase();
        self.triggers
            .iter()
            .any(|t| msg_lower.contains(&t.to_lowercase()))
    }
}

/// Recursively walk a directory collecting only executable paths (scripts/, bin/, binary).
///
/// Used by `ExecuteTool::extract_resources()` for sealed skills: only copy
/// executables to the temp dir — SKILL.md, references/, assets/ stay sealed.
pub fn walk_resources_filtered(base: &Path, dir: &Path, out: &mut Vec<String>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            // Only recurse into executable directories
            if matches!(name_str.as_ref(), "scripts" | "bin") {
                walk_resources_filtered(base, &path, out);
            }
        } else if let Ok(rel) = path.strip_prefix(base) {
            let rel_str = rel.to_string_lossy().to_string();
            // Only include executables
            if rel_str == "binary" || rel_str == "app"
                || rel_str.starts_with("scripts/") || rel_str.starts_with("bin/")
            {
                out.push(rel_str);
            }
        }
    }
}

/// Recursively walk a directory collecting relative paths, skipping SKILL.md and hidden files.
fn walk_resources(base: &Path, dir: &Path, out: &mut Vec<String>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        // Skip hidden files, SKILL.md, and packaging metadata
        if name_str.starts_with('.')
            || name_str.eq_ignore_ascii_case("skill.md")
            || name_str == "manifest.json"
            || name_str == "signatures.json"
        {
            continue;
        }
        if path.is_dir() {
            walk_resources(base, &path, out);
        } else if let Ok(rel) = path.strip_prefix(base) {
            out.push(rel.to_string_lossy().to_string());
        }
    }
}

/// Check if a tar entry name is metadata/packaging (should be excluded from resource listings).
fn is_metadata_entry(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower == "skill.md"
        || lower == "manifest.json"
        || lower == "signatures.json"
        || lower.starts_with('.')
}

/// Get the current platform name matching the Go convention.
fn current_platform() -> String {
    match std::env::consts::OS {
        "macos" => "macos".to_string(),
        other => other.to_string(),
    }
}

/// Split YAML frontmatter from a markdown document.
/// Returns (frontmatter_bytes, body_bytes).
/// Frontmatter is delimited by `---` on its own line.
pub fn split_frontmatter(data: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
    let text = std::str::from_utf8(data).map_err(|e| format!("invalid UTF-8: {}", e))?;
    let trimmed = text.trim_start();

    if !trimmed.starts_with("---") {
        return Err("no YAML frontmatter found (must start with ---)".into());
    }

    // Find the closing ---
    let after_first = &trimmed[3..];
    // Skip past the first newline after opening ---
    let start = after_first.find('\n').map(|i| i + 1).unwrap_or(0);
    let rest = &after_first[start..];

    // Find closing --- on its own line
    let close_pos = rest
        .find("\n---")
        .ok_or_else(|| "no closing --- found for frontmatter".to_string())?;

    let frontmatter = &rest[..close_pos];
    let body_start = close_pos + 4; // skip \n---
    let body = if body_start < rest.len() {
        let b = &rest[body_start..];
        b.strip_prefix('\n').unwrap_or(b)
    } else {
        ""
    };

    Ok((
        frontmatter.as_bytes().to_vec(),
        body.trim().as_bytes().to_vec(),
    ))
}

/// Parse a SKILL.md file into a Skill struct (frontmatter + body).
pub fn parse_skill_md(data: &[u8]) -> Result<Skill, String> {
    let (frontmatter, body) = split_frontmatter(data)?;

    let mut skill: Skill =
        serde_yaml::from_slice(&frontmatter).map_err(|e| format!("YAML parse error: {}", e))?;

    skill.template = String::from_utf8_lossy(&body).to_string();
    skill.validate()?;
    Ok(skill)
}

/// Parse only the YAML frontmatter of a SKILL.md file, skipping the body.
///
/// Returns a Skill with an empty template. Use this for metadata-only loading
/// (search, list, catalog) where the template body is not needed. The template
/// can be loaded lazily via `Loader::get()` when actually required.
pub fn parse_skill_frontmatter(data: &[u8]) -> Result<Skill, String> {
    let (frontmatter, _body) = split_frontmatter(data)?;

    let skill: Skill =
        serde_yaml::from_slice(&frontmatter).map_err(|e| format!("YAML parse error: {}", e))?;

    // template stays as default empty string (from #[serde(skip)])
    skill.validate()?;
    Ok(skill)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_SKILL: &str = r#"---
name: research
description: Deep research and information gathering
version: "0.2.0"
priority: 10
max_turns: 8
triggers:
  - research
  - find information
  - look up
platform:
  - macos
  - linux
tags:
  - research
  - information
metadata:
  nebo:
    emoji: "mag"
---

You are a research specialist. When activated, focus on:

1. Breaking down the research question
2. Using web search to find authoritative sources
3. Synthesizing findings into a clear summary
"#;

    #[test]
    fn test_parse_skill_md() {
        let skill = parse_skill_md(SAMPLE_SKILL.as_bytes()).unwrap();
        assert_eq!(skill.name, "research");
        assert_eq!(skill.description, "Deep research and information gathering");
        assert_eq!(skill.version, "0.2.0");
        assert_eq!(skill.priority, 10);
        assert_eq!(skill.max_turns, 8);
        assert_eq!(
            skill.triggers,
            vec!["research", "find information", "look up"]
        );
        assert_eq!(skill.platform, vec!["macos", "linux"]);
        assert!(skill.template.contains("research specialist"));
    }

    #[test]
    fn test_split_frontmatter() {
        let (fm, body) = split_frontmatter(SAMPLE_SKILL.as_bytes()).unwrap();
        let fm_str = std::str::from_utf8(&fm).unwrap();
        assert!(fm_str.contains("name: research"));
        let body_str = std::str::from_utf8(&body).unwrap();
        assert!(body_str.contains("research specialist"));
    }

    #[test]
    fn test_split_frontmatter_missing() {
        let result = split_frontmatter(b"no frontmatter here");
        assert!(result.is_err());
    }

    #[test]
    fn test_skill_validate() {
        let mut skill = Skill {
            name: String::new(),
            description: "test".into(),
            version: "1.0.0".into(),
            license: String::new(),
            compatibility: String::new(),
            allowed_tools: String::new(),
            author: String::new(),
            dependencies: vec![],
            tags: vec![],
            platform: vec![],
            triggers: vec![],
            capabilities: vec![],
            priority: 0,
            max_turns: 0,
            plugins: vec![],
            requires: vec![],
            metadata: HashMap::new(),
            template: String::new(),
            enabled: true,
            degraded: None,
            source_path: None,
            source: SkillSource::User,
            base_dir: None,
            napp_path: None,
            license_key: None,
        };
        assert!(skill.validate().is_err());
        skill.name = "test".into();
        assert!(skill.validate().is_ok());
    }

    #[test]
    fn test_matches_trigger() {
        let skill = parse_skill_md(SAMPLE_SKILL.as_bytes()).unwrap();
        assert!(skill.matches_trigger("Can you research the history of AI?"));
        assert!(skill.matches_trigger("Please FIND INFORMATION about Rust"));
        assert!(!skill.matches_trigger("Hello world"));
    }

    #[test]
    fn test_empty_platform_matches_all() {
        let mut skill = parse_skill_md(SAMPLE_SKILL.as_bytes()).unwrap();
        skill.platform.clear();
        assert!(skill.matches_platform());
    }

    #[test]
    fn test_capabilities_parsing() {
        let md = r#"---
name: xlsx-processor
description: Create Excel files
capabilities:
  - python
  - storage
---

Process spreadsheets.
"#;
        let skill = parse_skill_md(md.as_bytes()).unwrap();
        assert_eq!(skill.capabilities, vec!["python", "storage"]);
    }

    #[test]
    fn test_needs_sandbox() {
        let mut skill = parse_skill_md(SAMPLE_SKILL.as_bytes()).unwrap();
        assert!(!skill.needs_sandbox());

        skill.capabilities = vec!["python".into()];
        assert!(skill.needs_sandbox());

        skill.capabilities = vec!["typescript".into()];
        assert!(skill.needs_sandbox());

        skill.capabilities = vec!["storage".into(), "vision".into()];
        assert!(!skill.needs_sandbox());
    }

    #[test]
    fn test_list_resources_loose() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path();

        // Create skill structure
        std::fs::write(base.join("SKILL.md"), "---\nname: test\ndescription: t\n---\nbody").unwrap();
        std::fs::create_dir_all(base.join("scripts")).unwrap();
        std::fs::write(base.join("scripts/run.py"), "print('hello')").unwrap();
        std::fs::create_dir_all(base.join("references")).unwrap();
        std::fs::write(base.join("references/guide.md"), "# Guide").unwrap();

        let mut skill = parse_skill_md(b"---\nname: test\ndescription: t\n---\nbody").unwrap();
        skill.base_dir = Some(base.to_path_buf());

        let resources = skill.list_resources().unwrap();
        assert_eq!(resources.len(), 2);
        assert!(resources.iter().any(|r| r.contains("run.py")));
        assert!(resources.iter().any(|r| r.contains("guide.md")));
    }

    #[test]
    fn test_read_resource_loose() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path();
        std::fs::create_dir_all(base.join("scripts")).unwrap();
        std::fs::write(base.join("scripts/run.py"), "print('hello')").unwrap();

        let mut skill = parse_skill_md(b"---\nname: test\ndescription: t\n---\nbody").unwrap();
        skill.base_dir = Some(base.to_path_buf());

        let content = skill.read_resource("scripts/run.py").unwrap();
        assert_eq!(content, b"print('hello')");
    }

    #[test]
    fn test_read_resource_path_traversal() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut skill = parse_skill_md(b"---\nname: test\ndescription: t\n---\nbody").unwrap();
        skill.base_dir = Some(tmp.path().to_path_buf());

        let result = skill.read_resource("../../../etc/passwd");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("path traversal"));
    }

    // ── Agent Skills Standard Compliance Tests ──────────────────────

    #[test]
    fn test_standard_fields_parsing() {
        let md = r#"---
name: pdf-processing
description: Extract text and tables from PDF files.
license: Apache-2.0
compatibility: Requires python3, poppler-utils
allowed-tools: Bash(git:*) Read
metadata:
  author: example-org
  version: "1.0"
---

Process PDFs here.
"#;
        let skill = parse_skill_md(md.as_bytes()).unwrap();
        assert_eq!(skill.name, "pdf-processing");
        assert_eq!(skill.license, "Apache-2.0");
        assert_eq!(skill.compatibility, "Requires python3, poppler-utils");
        assert_eq!(skill.allowed_tools, "Bash(git:*) Read");
        assert_eq!(
            skill.metadata.get("author").and_then(|v| v.as_str()),
            Some("example-org")
        );
    }

    #[test]
    fn test_name_validation_uppercase() {
        let result = parse_skill_md(b"---\nname: PDF-Processing\ndescription: t\n---\nbody");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("lowercase"));
    }

    #[test]
    fn test_name_validation_leading_hyphen() {
        let result = parse_skill_md(b"---\nname: -pdf\ndescription: t\n---\nbody");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("hyphen"));
    }

    #[test]
    fn test_name_validation_trailing_hyphen() {
        let result = parse_skill_md(b"---\nname: pdf-\ndescription: t\n---\nbody");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("hyphen"));
    }

    #[test]
    fn test_name_validation_consecutive_hyphens() {
        let result = parse_skill_md(b"---\nname: pdf--processing\ndescription: t\n---\nbody");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("consecutive"));
    }

    #[test]
    fn test_name_validation_too_long() {
        let long_name = "a".repeat(65);
        let md = format!("---\nname: {}\ndescription: t\n---\nbody", long_name);
        let result = parse_skill_md(md.as_bytes());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("64"));
    }

    #[test]
    fn test_name_validation_valid_names() {
        for name in &["a", "pdf-processing", "data-analysis", "code-review", "a1b2"] {
            let md = format!("---\nname: {}\ndescription: test\n---\nbody", name);
            assert!(parse_skill_md(md.as_bytes()).is_ok(), "should accept name: {}", name);
        }
    }

    #[test]
    fn test_secrets_parsing() {
        let md = r#"---
name: brave-search
description: Web search via Brave
metadata:
  secrets:
    - key: BRAVE_API_KEY
      label: "Brave Search API Key"
      hint: "https://brave.com/search/api/"
      required: true
    - key: BRAVE_REGION
      label: "Default region"
      required: false
---

Search the web.
"#;
        let skill = parse_skill_md(md.as_bytes()).unwrap();
        let secrets = skill.secrets();
        assert_eq!(secrets.len(), 2);
        assert_eq!(secrets[0].key, "BRAVE_API_KEY");
        assert_eq!(secrets[0].label, "Brave Search API Key");
        assert_eq!(secrets[0].hint, "https://brave.com/search/api/");
        assert!(secrets[0].required);
        assert_eq!(secrets[1].key, "BRAVE_REGION");
        assert!(!secrets[1].required);
    }

    #[test]
    fn test_secrets_empty_when_not_declared() {
        let skill = parse_skill_md(SAMPLE_SKILL.as_bytes()).unwrap();
        assert!(skill.secrets().is_empty());
    }

    #[test]
    fn test_secrets_empty_with_non_array_metadata() {
        let md = r#"---
name: test-skill
description: test
metadata:
  secrets: "not an array"
---

body
"#;
        let skill = parse_skill_md(md.as_bytes()).unwrap();
        assert!(skill.secrets().is_empty());
    }
}
