use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// A skill parsed from a SKILL.md file with YAML frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
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
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub max_turns: i32,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    /// The markdown body (not from YAML — parsed from the content after frontmatter).
    #[serde(skip)]
    pub template: String,
    /// Whether this skill is currently enabled.
    #[serde(skip)]
    pub enabled: bool,
    /// Filesystem path this skill was loaded from.
    #[serde(skip)]
    pub source_path: Option<std::path::PathBuf>,
}

fn default_version() -> String {
    "1.0.0".to_string()
}

impl Skill {
    /// Validate that required fields are present.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("skill name is required".into());
        }
        if self.description.is_empty() {
            return Err("skill description is required".into());
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

/// Parse a SKILL.md file into a Skill struct.
pub fn parse_skill_md(data: &[u8]) -> Result<Skill, String> {
    let (frontmatter, body) = split_frontmatter(data)?;

    let mut skill: Skill =
        serde_yaml::from_slice(&frontmatter).map_err(|e| format!("YAML parse error: {}", e))?;

    skill.template = String::from_utf8_lossy(&body).to_string();
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
tools:
  - web
  - bot
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
        assert_eq!(skill.tools, vec!["web", "bot"]);
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
            author: String::new(),
            dependencies: vec![],
            tags: vec![],
            platform: vec![],
            triggers: vec![],
            tools: vec![],
            priority: 0,
            max_turns: 0,
            metadata: HashMap::new(),
            template: String::new(),
            enabled: true,
            source_path: None,
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
}
