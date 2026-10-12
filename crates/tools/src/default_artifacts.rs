//! The first-party artifacts every bot has: the ONE list of them.
//!
//! Each is installed in the background once the bot is signed in to NeboAI,
//! on every surface (the server's `default_artifacts`), and installed on the
//! spot, without asking, when a request needs one this bot lacks
//! ([`install_on_request`]). Both go through the one install door, the
//! marketplace code (`CodeInstaller`, the server's `codes::handle_code`), so
//! an install here is the install a store tap runs: auto-update on, removable
//! like any other. Adding a default is one line below.

use std::future::Future;
use std::time::Duration;

use tracing::{info, warn};

use crate::bot_tool::{ALREADY_INSTALLING, CodeInstaller, InstalledBy};

/// A first-party artifact every bot has: its marketplace slug (what its
/// plugin or skill folder is named) and the code it installs from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultArtifact {
    pub slug: &'static str,
    pub code: &'static str,
}

impl DefaultArtifact {
    /// A plugin (`PLUG-`), else a skill.
    pub fn is_plugin(&self) -> bool {
        self.code.starts_with("PLUG-")
    }
}

/// The defaults, in the order they install. One line each.
#[rustfmt::skip]
pub const DEFAULT_ARTIFACTS: &[DefaultArtifact] = &[
    DefaultArtifact { slug: "nebo-office", code: "PLUG-BHVY-A96N" },
    DefaultArtifact { slug: "nebo-media", code: "PLUG-1XD1-1QEV" },
    DefaultArtifact { slug: "ballast", code: "PLUG-SPP9-AAAG" },
    DefaultArtifact { slug: "nebo-design", code: "SKIL-VQTF-WV8E" },
    DefaultArtifact { slug: "neboai", code: "SKIL-TV64-VHQ4" },
];

/// The default named `slug`, when it is one.
pub fn default_artifact(slug: &str) -> Option<&'static DefaultArtifact> {
    DEFAULT_ARTIFACTS
        .iter()
        .find(|d| d.slug.eq_ignore_ascii_case(slug.trim()))
}

/// How long a request waits for a default another door is installing right
/// now (the background install, a store tap) to land.
const INSTALL_WAIT: Duration = Duration::from_secs(300);

/// The safety net: install default `d`, which a request needs and this bot
/// lacks, on the spot and without asking, so the turn that needed it goes on.
/// A default the owner removed comes back here too: the request needs it.
/// When another door is already installing it, this waits for that install
/// instead of starting a second. `landed` says whether it is installed now.
/// Returns whether it is; a failure is logged, and the caller goes on as it
/// would for any missing plugin or skill.
pub async fn install_on_request<F, Fut>(
    installer: &dyn CodeInstaller,
    d: &DefaultArtifact,
    by: InstalledBy,
    platform: Option<&str>,
    landed: F,
) -> bool
where
    F: Fn() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + INSTALL_WAIT;
    let mut line = installer.install(d.code, by, platform).await;
    // Another door holds the code: wait for its install, asking the door
    // again now and then. Once that install has ended without the default
    // landing, the door is free and this asks for the install itself.
    while line.contains(ALREADY_INSTALLING) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if landed().await {
            break;
        }
        line = installer.install(d.code, by, platform).await;
    }
    let installed = landed().await;
    if installed {
        info!(
            slug = d.slug,
            "default installed for a request that needed it"
        );
    } else {
        warn!(slug = d.slug, outcome = %line, "a request needed a default that did not install");
    }
    installed
}

/// A stand-in for the install door, for the tool tests: it counts the codes
/// it is handed and puts a default plugin on disk the way the real door
/// does; any other code fails.
#[cfg(test)]
pub(crate) struct FakeDoor {
    plugins: std::path::PathBuf,
    pub(crate) calls: std::sync::Mutex<Vec<String>>,
}

#[cfg(test)]
impl FakeDoor {
    /// A door that installs into the plugin store rooted at `plugins`.
    pub(crate) fn new(plugins: &std::path::Path) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            plugins: plugins.to_path_buf(),
            calls: Default::default(),
        })
    }

    pub(crate) fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl CodeInstaller for FakeDoor {
    fn install<'a>(
        &'a self,
        code: &'a str,
        _by: InstalledBy,
        _platform: Option<&'a str>,
    ) -> std::pin::Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(code.to_string());
            let Some(d) = DEFAULT_ARTIFACTS
                .iter()
                .find(|d| d.code == code && d.is_plugin())
            else {
                return format!("Couldn't install {code}: the code isn't valid.");
            };
            let dir = self.plugins.join(d.slug).join("0.1.0");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("plugin.json"),
                serde_json::json!({"id": d.slug, "slug": d.slug, "name": d.slug, "version": "0.1.0", "platforms": {}})
                    .to_string(),
            )
            .unwrap();
            let bin = dir.join(d.slug);
            std::fs::write(&bin, b"#!/bin/sh\necho ok\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            format!("Installed plugin: {}", d.slug)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_names_each_default_once_with_a_code_of_its_kind() {
        let mut seen = std::collections::HashSet::new();
        for d in DEFAULT_ARTIFACTS {
            assert!(seen.insert(d.slug), "{} is listed twice", d.slug);
            assert!(
                d.code.starts_with("PLUG-") || d.code.starts_with("SKIL-"),
                "{}: {}",
                d.slug,
                d.code
            );
        }
        assert_eq!(
            default_artifact("Nebo-Office").map(|d| d.code),
            Some("PLUG-BHVY-A96N")
        );
        assert!(default_artifact("quickbooks").is_none());
        assert!(default_artifact("neboai").is_some_and(|d| !d.is_plugin()));
    }
}
