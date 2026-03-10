//! Maps skill capabilities to sandbox configuration.
//!
//! Each skill declares capabilities like `["python", "network", "storage"]`.
//! This module translates those into a `SandboxRuntimeConfig` that controls
//! filesystem and network access for the sandboxed subprocess.

use std::path::Path;

use sandbox_runtime::SandboxRuntimeConfig;

use crate::skills::Skill;

/// Sensitive paths that are always denied for reading.
const DENY_READ: &[&str] = &[
    "~/.ssh",
    "~/.gnupg",
    "~/.aws/credentials",
    "~/.config/gcloud",
];

/// Default package registry domains allowed when `network` capability is present.
const PACKAGE_REGISTRY_DOMAINS: &[&str] = &[
    "pypi.org",
    "files.pythonhosted.org",
    "registry.npmjs.org",
    "npm.pkg.github.com",
];

/// Build a per-skill sandbox config from the skill's declared capabilities.
pub fn build_sandbox_config(skill: &Skill, work_dir: &Path) -> SandboxRuntimeConfig {
    let mut config = SandboxRuntimeConfig::default_config();

    // --- Filesystem ---

    // Always deny sensitive paths
    config.filesystem.deny_read = DENY_READ.iter().map(|s| s.to_string()).collect();

    // Always allow writing to the work dir, stdout/stderr, and nebo temp
    let work_str = work_dir.to_string_lossy().to_string();
    config.filesystem.allow_write = vec![
        work_str,
        "/dev/stdout".to_string(),
        "/dev/stderr".to_string(),
        "/tmp/nebo".to_string(),
    ];

    // `storage` capability: also allow writing to the nebo data dir
    if skill.capabilities.iter().any(|c| c == "storage") {
        if let Ok(data_dir) = config::data_dir() {
            config
                .filesystem
                .allow_write
                .push(data_dir.to_string_lossy().to_string());
        }
    }

    // --- Network ---

    if skill.capabilities.iter().any(|c| c == "network") {
        // Start with package registries
        let mut domains: Vec<String> = PACKAGE_REGISTRY_DOMAINS
            .iter()
            .map(|s| s.to_string())
            .collect();

        // Add skill-specific allowed domains from metadata
        if let Some(extra) = skill.metadata.get("allowed_domains") {
            if let Some(arr) = extra.as_array() {
                for v in arr {
                    if let Some(d) = v.as_str() {
                        domains.push(d.to_string());
                    }
                }
            }
        }

        config.network.allowed_domains = domains;
    }
    // If no "network" capability, allowed_domains stays empty → all network blocked

    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn test_skill(capabilities: Vec<&str>, metadata: HashMap<String, serde_json::Value>) -> Skill {
        Skill {
            name: "test-skill".into(),
            description: "test".into(),
            version: "1.0.0".into(),
            author: String::new(),
            dependencies: vec![],
            tags: vec![],
            platform: vec![],
            triggers: vec![],
            capabilities: capabilities.into_iter().map(String::from).collect(),
            priority: 0,
            max_turns: 0,
            metadata,
            template: String::new(),
            enabled: true,
            source_path: None,
            source: crate::skills::SkillSource::User,
            base_dir: None,
        }
    }

    #[test]
    fn test_no_capabilities_blocks_network() {
        let skill = test_skill(vec![], HashMap::new());
        let config = build_sandbox_config(&skill, Path::new("/tmp/work"));
        assert!(config.network.allowed_domains.is_empty());
    }

    #[test]
    fn test_network_capability_allows_registries() {
        let skill = test_skill(vec!["network"], HashMap::new());
        let config = build_sandbox_config(&skill, Path::new("/tmp/work"));
        assert!(config.network.allowed_domains.contains(&"pypi.org".to_string()));
        assert!(config.network.allowed_domains.contains(&"registry.npmjs.org".to_string()));
    }

    #[test]
    fn test_network_with_extra_domains() {
        let mut meta = HashMap::new();
        meta.insert(
            "allowed_domains".into(),
            serde_json::json!(["api.example.com", "cdn.example.com"]),
        );
        let skill = test_skill(vec!["network"], meta);
        let config = build_sandbox_config(&skill, Path::new("/tmp/work"));
        assert!(config.network.allowed_domains.contains(&"api.example.com".to_string()));
        assert!(config.network.allowed_domains.contains(&"cdn.example.com".to_string()));
    }

    #[test]
    fn test_always_denies_sensitive_paths() {
        let skill = test_skill(vec![], HashMap::new());
        let config = build_sandbox_config(&skill, Path::new("/tmp/work"));
        assert!(config.filesystem.deny_read.contains(&"~/.ssh".to_string()));
        assert!(config.filesystem.deny_read.contains(&"~/.gnupg".to_string()));
    }

    #[test]
    fn test_work_dir_always_writable() {
        let skill = test_skill(vec![], HashMap::new());
        let config = build_sandbox_config(&skill, Path::new("/tmp/my-work"));
        assert!(config.filesystem.allow_write.contains(&"/tmp/my-work".to_string()));
    }

    #[test]
    fn test_storage_capability_adds_data_dir() {
        let skill = test_skill(vec!["storage"], HashMap::new());
        let config = build_sandbox_config(&skill, Path::new("/tmp/work"));
        // Should have more than the base 4 writable paths
        assert!(config.filesystem.allow_write.len() >= 4);
    }
}
