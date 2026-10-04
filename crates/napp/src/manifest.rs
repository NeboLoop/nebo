use serde::{Deserialize, Serialize};

use crate::NappError;

/// Parsed qualified name: `@org/type/name`.
///
/// Valid types: `skills`, `workflows`, `agents`.
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
        if !["skills", "workflows", "agents"].contains(&artifact_type) {
            return Err(NappError::Manifest(format!(
                "invalid artifact type '{}' in qualified name (expected skills/workflows/agents)",
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
        format!(
            "@{}/{}/{}",
            self.org, self.artifact_type, self.artifact_name
        )
    }
}

/// Package manifest (manifest.json) — universal envelope for all artifact types.
///
/// Every artifact (skill, tool, workflow, agent) includes a manifest.json with
/// identity fields (id, name, version, type, description). Tool-specific fields
/// (provides, permissions, implements, etc.) default to empty for non-tool artifacts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub version: String,
    /// Artifact type: "skill", "tool", "workflow", or "agent".
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
    /// Window configuration for app-type agents.
    #[serde(default)]
    pub window: Option<AppWindowConfig>,
    /// Optional setup wizard declared by the artifact. The frontend
    /// renders this as a multi-step flow with Form / Generate /
    /// External / Credentials steps. See `crate::plugin::ArtifactSetup`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<crate::plugin::ArtifactSetup>,
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

/// Window configuration for app-type agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppWindowConfig {
    #[serde(default = "default_window_width")]
    pub width: u32,
    #[serde(default = "default_window_height")]
    pub height: u32,
    #[serde(default = "default_true")]
    pub resizable: bool,
    #[serde(default)]
    pub title: Option<String>,
    /// The page takes the whole screen: on the phone, no app bar, no safe
    /// area (the page pads itself with `env(safe-area-inset-*)`), the system
    /// bars hidden and the screen kept awake. A game asks for this.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fullscreen: bool,
    /// `portrait` (the default), `landscape` or `any`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orientation: Option<String>,
    /// `true` gives this app's page the phone's pull-down-to-reload. Off
    /// unless asked for: an app is used by touch, and a downward drag that
    /// reloads it (a design canvas, a card game) makes it unusable.
    /// Fullscreen apps never have it.
    #[serde(default, alias = "pullToRefresh", skip_serializing_if = "std::ops::Not::not")]
    pub pull_to_refresh: bool,
    /// `true` puts the chat's dictate and voice buttons in the app's bar on
    /// the phone, so the owner talks to the employee without leaving the
    /// app. Never shown unless asked for. Fullscreen apps have no bar.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub voice: bool,
    /// `true` opens the app over its chat, on the phone and the desktop, as
    /// soon as the employee writes this chat's record (a `chat:` key in
    /// `app_data`), so the owner watches the work happen without pressing
    /// Open App. Off unless asked for. Fullscreen apps may ask for it too.
    #[serde(default, alias = "openOnWork", skip_serializing_if = "std::ops::Not::not")]
    pub open_on_work: bool,
    /// `true` serves the app's page and every file it loads cross-origin
    /// isolated (`Cross-Origin-Opener-Policy: same-origin`,
    /// `Cross-Origin-Embedder-Policy: require-corp`), which gives it
    /// `SharedArrayBuffer`: a threaded WebAssembly engine export (Godot 4,
    /// Unity, Bevy) needs it. The page can then load nothing from another
    /// site (a CDN script, esm.sh, a remote image): everything it uses ships
    /// in `ui/`. Off unless asked for.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub isolated: bool,
}

/// The orientations a window may ask for; the first is the default.
pub const WINDOW_ORIENTATIONS: &[&str] = &["portrait", "landscape", "any"];

/// The permission an app declares to read the gyroscope and accelerometer.
pub const DEVICE_MOTION: &str = "device:motion";

impl AppWindowConfig {
    /// Refuse an orientation no view knows how to show.
    pub fn validate(&self) -> Result<(), NappError> {
        match self.orientation.as_deref() {
            Some(o) if !WINDOW_ORIENTATIONS.contains(&o) => Err(NappError::Manifest(format!(
                "window.orientation must be one of {}, not {o:?}",
                WINDOW_ORIENTATIONS.join(", ")
            ))),
            _ => Ok(()),
        }
    }
}

/// How an app asks to be shown, as every client reads it (`appWindow` on an
/// employee): the manifest's `window.fullscreen`, `window.orientation` and
/// `window.pull_to_refresh`, `window.voice`, `window.open_on_work` and `window.isolated`, and whether its permissions include `device:motion`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppWindow {
    pub fullscreen: bool,
    pub orientation: &'static str,
    pub motion: bool,
    /// Whether the phone offers pull-down-to-reload on the page.
    #[serde(rename = "pullToRefresh")]
    pub pull_to_refresh: bool,
    /// Whether the phone's app bar carries the dictate and voice buttons.
    pub voice: bool,
    /// Whether the app opens over its chat when the employee writes that
    /// chat's record.
    #[serde(rename = "openOnWork")]
    pub open_on_work: bool,
    /// Whether the app's page is served cross-origin isolated.
    pub isolated: bool,
}

impl AppWindow {
    /// From the manifest's window block (absent = the defaults) and its
    /// permissions. An orientation no view knows reads as portrait.
    pub fn from_manifest(window: Option<&AppWindowConfig>, permissions: &[String]) -> Self {
        let orientation = window
            .and_then(|w| w.orientation.as_deref())
            .and_then(|o| WINDOW_ORIENTATIONS.iter().find(|k| **k == o).copied())
            .unwrap_or(WINDOW_ORIENTATIONS[0]);
        let fullscreen = window.is_some_and(|w| w.fullscreen);
        Self {
            fullscreen,
            orientation,
            motion: permissions.iter().any(|p| p == DEVICE_MOTION),
            pull_to_refresh: !fullscreen && window.is_some_and(|w| w.pull_to_refresh),
            voice: !fullscreen && window.is_some_and(|w| w.voice),
            open_on_work: window.is_some_and(|w| w.open_on_work),
            isolated: window.is_some_and(|w| w.isolated),
        }
    }
}

fn default_window_width() -> u32 {
    1024
}
fn default_window_height() -> u32 {
    768
}
fn default_true() -> bool {
    true
}

impl Default for AppWindowConfig {
    fn default() -> Self {
        Self {
            width: 1024,
            height: 768,
            resizable: true,
            title: None,
            fullscreen: false,
            orientation: None,
            pull_to_refresh: false,
            voice: false,
            open_on_work: false,
            isolated: false,
        }
    }
}

/// Valid capabilities a tool can provide.
const VALID_CAPABILITIES: &[&str] = &[
    "gateway", "vision", "browser", "comm", "ui", "schedule", "hooks",
];

/// Valid permission prefixes.
const VALID_PERMISSION_PREFIXES: &[&str] = &[
    "network:",
    "filesystem:",
    "settings:",
    "capability:",
    "memory:",
    "session:",
    "context:",
    "tool:",
    "shell:",
    "subagent:",
    "lane:",
    "channel:",
    "comm:",
    "notification:",
    "embedding:",
    "skill:",
    "advisor:",
    "model:",
    "mcp:",
    "database:",
    "storage:",
    "schedule:",
    "voice:",
    "browser:",
    "oauth:",
    "user:",
    "hook:",
    "device:",
];

/// Every permission must carry a known prefix. The one check for a
/// manifest's `permissions`, whether it arrives as a file (`validate`) or as
/// the `app.permissions` a tool call is about to write.
pub fn validate_permissions(permissions: &[String]) -> Result<(), NappError> {
    for perm in permissions {
        let valid = VALID_PERMISSION_PREFIXES
            .iter()
            .any(|prefix| perm.starts_with(prefix));
        if !valid {
            return Err(NappError::Manifest(format!("unknown permission: {}", perm)));
        }
    }
    Ok(())
}

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
                return Err(NappError::Manifest(format!("unknown capability: {}", cap)));
            }
        }

        validate_permissions(&self.permissions)?;
        if let Some(window) = &self.window {
            window.validate()?;
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
            p == perm
                || (p.ends_with(':') && perm.starts_with(p))
                || p == &format!("{}:*", perm.split(':').next().unwrap_or(""))
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

    // The window block a game declares, read the way every client gets it.
    #[test]
    fn app_window_reads_the_window_block_and_device_motion() {
        let m: Manifest = serde_json::from_str(
            r#"{"id":"kart","name":"Kart","version":"1.0.0","type":"app",
                "permissions":["storage:readwrite","device:motion"],
                "window":{"title":"Kart","width":420,"height":800,"resizable":false,"fullscreen":true,"orientation":"landscape"}}"#,
        )
        .unwrap();
        m.validate().unwrap();
        let w = AppWindow::from_manifest(m.window.as_ref(), &m.permissions);
        assert_eq!(w, AppWindow { fullscreen: true, orientation: "landscape", motion: true, pull_to_refresh: false, voice: false, open_on_work: false, isolated: false });
        assert_eq!(
            serde_json::to_value(&w).unwrap(),
            serde_json::json!({"fullscreen": true, "orientation": "landscape", "motion": true, "pullToRefresh": false, "voice": false, "openOnWork": false, "isolated": false})
        );

        // Missing = today's view: not fullscreen, portrait, no motion, and no
        // pull-to-refresh unless the app asks for it.
        let today = AppWindow::from_manifest(None, &[]);
        assert_eq!(today, AppWindow { fullscreen: false, orientation: "portrait", motion: false, pull_to_refresh: false, voice: false, open_on_work: false, isolated: false });
        let asks: AppWindowConfig = serde_json::from_str(r#"{"pull_to_refresh":true}"#).unwrap();
        assert!(AppWindow::from_manifest(Some(&asks), &[]).pull_to_refresh);
        let plain: AppWindowConfig = serde_json::from_str(r#"{"title":"Deals"}"#).unwrap();
        assert_eq!(AppWindow::from_manifest(Some(&plain), &["storage:readwrite".into()]), today);

        // An orientation no view knows reads as portrait, and a write refuses it.
        let odd: AppWindowConfig = serde_json::from_str(r#"{"orientation":"sideways"}"#).unwrap();
        assert_eq!(AppWindow::from_manifest(Some(&odd), &[]).orientation, "portrait");
        assert!(odd.validate().is_err());

        // Pull-to-reload is off unless an app turns it on (snake_case or
        // camelCase key); turned on, it survives being written back.
        for json in [r#"{"pull_to_refresh":true}"#, r#"{"pullToRefresh":true}"#] {
            let pull: AppWindowConfig = serde_json::from_str(json).unwrap();
            let w = AppWindow::from_manifest(Some(&pull), &[]);
            assert!(!w.fullscreen && w.pull_to_refresh);
            assert_eq!(serde_json::to_value(&pull).unwrap()["pull_to_refresh"], true);
        }
        let off: AppWindowConfig = serde_json::from_str(r#"{"pull_to_refresh":false}"#).unwrap();
        assert!(!AppWindow::from_manifest(Some(&off), &[]).pull_to_refresh);
        // Voice buttons only when asked for, and never on a fullscreen app.
        assert!(!AppWindow::from_manifest(Some(&off), &[]).voice);
        let voice: AppWindowConfig = serde_json::from_str(r#"{"voice":true}"#).unwrap();
        assert!(AppWindow::from_manifest(Some(&voice), &[]).voice);
        let game: AppWindowConfig = serde_json::from_str(r#"{"voice":true,"fullscreen":true}"#).unwrap();
        assert!(!AppWindow::from_manifest(Some(&game), &[]).voice);
        // Opening on work only when asked for (either key spelling), and a
        // fullscreen app may ask for it too; it survives being written back.
        assert!(!AppWindow::from_manifest(Some(&off), &[]).open_on_work);
        for json in [r#"{"open_on_work":true}"#, r#"{"openOnWork":true,"fullscreen":true}"#] {
            let work: AppWindowConfig = serde_json::from_str(json).unwrap();
            assert!(AppWindow::from_manifest(Some(&work), &[]).open_on_work);
            assert_eq!(serde_json::to_value(&work).unwrap()["open_on_work"], true);
        }
        // Cross-origin isolation only when asked for (a threaded engine
        // export); it survives being written back.
        assert!(!AppWindow::from_manifest(Some(&off), &[]).isolated);
        let iso: AppWindowConfig = serde_json::from_str(r#"{"isolated":true,"fullscreen":true}"#).unwrap();
        assert!(AppWindow::from_manifest(Some(&iso), &[]).isolated);
        assert_eq!(serde_json::to_value(&iso).unwrap()["isolated"], true);

        // A window written back keeps its old shape when the new keys are unset.
        assert_eq!(
            serde_json::to_value(&plain).unwrap(),
            serde_json::json!({"width": 1024, "height": 768, "resizable": true, "title": "Deals"})
        );
    }

    /// The permission check a tool call uses is the manifest's own: a known
    /// prefix passes, anything else is named in the error.
    #[test]
    fn validate_permissions_is_the_manifest_rule() {
        validate_permissions(&["storage:readwrite".into(), "network:outbound".into()]).unwrap();
        let err = validate_permissions(&["bogus:thing".into()]).unwrap_err();
        assert_eq!(err.to_string(), NappError::Manifest("unknown permission: bogus:thing".into()).to_string());
    }

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
            window: None,
            setup: None,
        }
    }
}
