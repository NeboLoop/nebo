use serde::{Deserialize, Serialize};

use crate::NappError;

/// Tool manifest (manifest.json in .napp package).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_runtime")]
    pub runtime: String,
    #[serde(default = "default_protocol")]
    pub protocol: String,
    #[serde(default)]
    pub signature: Option<ManifestSignature>,
    #[serde(default)]
    pub startup_timeout: u32,
    #[serde(default)]
    pub provides: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub overrides: Vec<String>,
    #[serde(default)]
    pub oauth: Vec<OAuthRequirement>,
    #[serde(default)]
    pub implements: Vec<String>,
}

fn default_runtime() -> String {
    "local".to_string()
}
fn default_protocol() -> String {
    "grpc".to_string()
}

/// Code signing info in the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestSignature {
    #[serde(default)]
    pub algorithm: String,
    #[serde(default)]
    pub key_id: String,
}

/// OAuth requirement for a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthRequirement {
    pub provider: String,
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// Valid capabilities a tool can provide.
const VALID_CAPABILITIES: &[&str] = &[
    "gateway", "vision", "browser", "comm", "ui", "schedule", "hooks",
];

/// Valid permission prefixes.
const VALID_PERMISSION_PREFIXES: &[&str] = &[
    "network:", "filesystem:", "settings:", "capability:", "memory:",
    "session:", "context:", "tool:", "shell:", "subagent:", "lane:",
    "channel:", "comm:", "notification:", "embedding:", "skill:",
    "advisor:", "model:", "mcp:", "database:", "storage:", "schedule:",
    "voice:", "browser:", "oauth:", "user:", "hook:",
];

impl Manifest {
    /// Load manifest from a JSON file.
    pub fn load(path: &std::path::Path) -> Result<Self, NappError> {
        let data = std::fs::read_to_string(path)?;
        let manifest: Self = serde_json::from_str(&data)?;
        Ok(manifest)
    }

    /// Validate the manifest.
    pub fn validate(&self) -> Result<(), NappError> {
        if self.id.is_empty() {
            return Err(NappError::Manifest("id is required".into()));
        }
        if self.name.is_empty() {
            return Err(NappError::Manifest("name is required".into()));
        }
        if self.version.is_empty() {
            return Err(NappError::Manifest("version is required".into()));
        }

        // Validate capabilities
        for cap in &self.provides {
            let base = cap.split(':').next().unwrap_or(cap);
            if base != "tool" && base != "channel" && !VALID_CAPABILITIES.contains(&base) {
                return Err(NappError::Manifest(format!(
                    "unknown capability: {}",
                    cap
                )));
            }
        }

        // Validate permissions
        for perm in &self.permissions {
            let valid = VALID_PERMISSION_PREFIXES
                .iter()
                .any(|prefix| perm.starts_with(prefix));
            if !valid {
                return Err(NappError::Manifest(format!(
                    "unknown permission: {}",
                    perm
                )));
            }
        }

        // Overrides require hook: permission
        for override_name in &self.overrides {
            let required_perm = format!("hook:{}", override_name);
            if !self.permissions.contains(&required_perm) {
                return Err(NappError::Manifest(format!(
                    "override '{}' requires permission '{}'",
                    override_name, required_perm
                )));
            }
        }

        // Startup timeout cap
        if self.startup_timeout > 120 {
            return Err(NappError::Manifest(
                "startup_timeout must be <= 120 seconds".into(),
            ));
        }

        Ok(())
    }

    /// Check if the tool has a specific permission.
    pub fn has_permission(&self, perm: &str) -> bool {
        self.permissions.iter().any(|p| {
            p == perm || (p.ends_with(':') && perm.starts_with(p)) || p == &format!("{}:*", perm.split(':').next().unwrap_or(""))
        })
    }

    /// Check if the tool has a permission with a prefix (wildcard support).
    pub fn has_permission_prefix(&self, prefix: &str) -> bool {
        self.permissions.iter().any(|p| p.starts_with(prefix))
    }

    /// Get the effective startup timeout (default 10s).
    pub fn effective_startup_timeout(&self) -> u32 {
        if self.startup_timeout == 0 {
            10
        } else {
            self.startup_timeout.min(120)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_valid_manifest() {
        let m = Manifest {
            id: "test-tool".into(),
            name: "Test Tool".into(),
            version: "1.0.0".into(),
            description: "A test tool".into(),
            runtime: "local".into(),
            protocol: "grpc".into(),
            signature: None,
            startup_timeout: 10,
            provides: vec!["gateway".into(), "tool:search".into()],
            permissions: vec!["network:*".into(), "tool:web".into()],
            overrides: vec![],
            oauth: vec![],
            implements: vec![],
        };
        assert!(m.validate().is_ok());
    }

    #[test]
    fn test_validate_missing_id() {
        let m = Manifest {
            id: "".into(),
            name: "Test".into(),
            version: "1.0.0".into(),
            ..Default::default()
        };
        assert!(m.validate().is_err());
    }

    #[test]
    fn test_has_permission() {
        let m = Manifest {
            id: "x".into(),
            name: "X".into(),
            version: "1".into(),
            permissions: vec!["network:*".into(), "tool:web".into()],
            ..Default::default()
        };
        assert!(m.has_permission("tool:web"));
        assert!(m.has_permission("network:example.com"));
    }

    #[test]
    fn test_startup_timeout() {
        let mut m = Manifest::default();
        m.id = "x".into();
        m.name = "X".into();
        m.version = "1".into();
        assert_eq!(m.effective_startup_timeout(), 10);
        m.startup_timeout = 60;
        assert_eq!(m.effective_startup_timeout(), 60);
        m.startup_timeout = 200;
        assert_eq!(m.effective_startup_timeout(), 120);
    }

    #[test]
    fn test_implements_field() {
        let json = r#"{
            "id": "crm-tool",
            "name": "CRM Tool",
            "version": "1.0.0",
            "implements": ["crm-lookup", "contact-search"]
        }"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.implements, vec!["crm-lookup", "contact-search"]);
    }
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            version: String::new(),
            description: String::new(),
            runtime: "local".into(),
            protocol: "grpc".into(),
            signature: None,
            startup_timeout: 0,
            provides: vec![],
            permissions: vec![],
            overrides: vec![],
            oauth: vec![],
            implements: vec![],
        }
    }
}
