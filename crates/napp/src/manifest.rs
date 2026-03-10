use serde::{Deserialize, Serialize};

use crate::NappError;

/// Parsed qualified name: `@org/type/name`.
///
/// Valid types: `skills`, `workflows`, `roles`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualifiedName {
    pub org: String,
    pub artifact_type: String,
    pub artifact_name: String,
}

impl QualifiedName {
    /// Parse `@org/type/name` format.
    pub fn parse(name: &str) -> Result<Self, NappError> {
        let s = name.strip_prefix('@').ok_or_else(|| {
            NappError::Manifest(format!("qualified name must start with '@': {}", name))
        })?;

        let parts: Vec<&str> = s.splitn(3, '/').collect();
        if parts.len() != 3 {
            return Err(NappError::Manifest(format!(
                "qualified name must be @org/type/name: {}",
                name
            )));
        }

        let artifact_type = parts[1];
        if !["skills", "workflows", "roles"].contains(&artifact_type) {
            return Err(NappError::Manifest(format!(
                "invalid artifact type '{}' in qualified name (expected skills/workflows/roles)",
                artifact_type
            )));
        }

        Ok(Self {
            org: parts[0].to_string(),
            artifact_type: artifact_type.to_string(),
            artifact_name: parts[2].to_string(),
        })
    }

    /// Format as the full qualified string.
    pub fn to_string(&self) -> String {
        format!("@{}/{}/{}", self.org, self.artifact_type, self.artifact_name)
    }
}

/// Package manifest (manifest.json) — universal envelope for all artifact types.
///
/// Every artifact (skill, tool, workflow, role) includes a manifest.json with
/// identity fields (id, name, version, type, description). Tool-specific fields
/// (provides, permissions, implements, etc.) default to empty for non-tool artifacts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    /// Artifact type: "skill", "tool", "workflow", or "role".
    #[serde(rename = "type", default)]
    pub artifact_type: String,
    #[serde(default)]
    pub description: String,
    /// Publisher name.
    #[serde(default)]
    pub author: String,
    /// Marketplace code (assigned on publish).
    #[serde(default)]
    pub code: String,
    /// Categorization tags.
    #[serde(default)]
    pub tags: Vec<String>,
    // -- Tool-specific fields (ignored for non-tool artifacts) --
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
    /// SHA256 hash of the binary — verified on every launch.
    #[serde(default)]
    pub binary_hash: String,
    /// Signature over the manifest content.
    #[serde(default)]
    pub manifest_signature: String,
    /// Signature over the binary hash.
    #[serde(default)]
    pub binary_signature: String,
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

    /// Backward-compatible ID accessor.
    ///
    /// If `name` is a qualified name (`@org/type/name`), returns `&self.name`.
    /// Otherwise returns `&self.id`.
    pub fn id(&self) -> &str {
        if self.name.starts_with('@') {
            &self.name
        } else {
            &self.id
        }
    }

    /// Parse the qualified name if `name` starts with `@`.
    pub fn qualified_name(&self) -> Option<QualifiedName> {
        if self.name.starts_with('@') {
            QualifiedName::parse(&self.name).ok()
        } else {
            None
        }
    }

    /// Validate the manifest.
    pub fn validate(&self) -> Result<(), NappError> {
        if self.id.is_empty() && !self.name.starts_with('@') {
            return Err(NappError::Manifest("id is required".into()));
        }
        if self.name.is_empty() {
            return Err(NappError::Manifest("name is required".into()));
        }
        if self.version.is_empty() {
            return Err(NappError::Manifest("version is required".into()));
        }

        // Validate qualified name format when name starts with @
        if self.name.starts_with('@') {
            QualifiedName::parse(&self.name)?;
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
            startup_timeout: 10,
            provides: vec!["gateway".into(), "tool:search".into()],
            permissions: vec!["network:*".into(), "tool:web".into()],
            ..Default::default()
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
    fn test_qualified_name_parse() {
        let qn = QualifiedName::parse("@acme/skills/crm-lookup").unwrap();
        assert_eq!(qn.org, "acme");
        assert_eq!(qn.artifact_type, "skills");
        assert_eq!(qn.artifact_name, "crm-lookup");
    }

    #[test]
    fn test_qualified_name_invalid() {
        assert!(QualifiedName::parse("not-qualified").is_err());
        assert!(QualifiedName::parse("@acme/invalid_type/name").is_err());
        assert!(QualifiedName::parse("@acme/skills").is_err());
        assert!(QualifiedName::parse("@acme/tools/name").is_err()); // tools no longer valid
    }

    #[test]
    fn test_manifest_id_accessor() {
        let mut m = Manifest::default();
        m.id = "legacy-id".into();
        m.name = "Legacy Name".into();
        m.version = "1.0".into();
        assert_eq!(m.id(), "legacy-id");

        m.name = "@acme/skills/crm-lookup".into();
        assert_eq!(m.id(), "@acme/skills/crm-lookup");
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
            artifact_type: String::new(),
            description: String::new(),
            author: String::new(),
            code: String::new(),
            tags: vec![],
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
