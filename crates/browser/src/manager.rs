use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::info;

use crate::BrowserError;
use crate::cdp_bridge::{CdpBridge, ObscuraConfig, find_obscura};
use crate::chrome::RunningChrome;
use crate::config::BrowserConfig;
use crate::executor::ActionExecutor;
use crate::extension_bridge::ExtensionBridge;
use crate::session::Session;

/// Manages browser instances and sessions.
pub struct Manager {
    config: BrowserConfig,
    data_dir: String,
    browsers: RwLock<HashMap<String, RunningChrome>>,
    sessions: RwLock<HashMap<String, Arc<Session>>>,
    bridge: Arc<ExtensionBridge>,
    /// Tier-2 built-in Chrome (CDP), launched lazily on first fallback use.
    cdp: Option<Arc<CdpBridge>>,
}

/// Status info for a browser profile.
pub struct ProfileStatus {
    pub name: String,
    pub driver: String,
    pub running: bool,
    pub page_count: usize,
    pub cdp_port: u16,
}

impl Manager {
    pub fn new(config: BrowserConfig, data_dir: String) -> Self {
        // Tier-2 built-in browser: the bundled Obscura headless browser, driven over CDP.
        // Launched lazily on first fallback use; None if the binary isn't found (→ tier-3 direct).
        let cdp = find_obscura(&data_dir).map(|binary| {
            Arc::new(CdpBridge::new(ObscuraConfig {
                binary,
                storage_dir: Some(PathBuf::from(&data_dir).join("obscura-profile")),
                stealth: true,
                log_path: Some(PathBuf::from(&data_dir).join("logs").join("obscura.log")),
            }))
        });
        Self {
            config,
            data_dir,
            browsers: RwLock::new(HashMap::new()),
            sessions: RwLock::new(HashMap::new()),
            bridge: Arc::new(ExtensionBridge::new()),
            cdp,
        }
    }

    /// Get the extension bridge for WS handler wiring.
    pub fn bridge(&self) -> Arc<ExtensionBridge> {
        self.bridge.clone()
    }

    /// Get an ActionExecutor: tier 1 extension → tier 2 built-in Chrome (CDP).
    ///
    /// This is the ONE place to ask about browser-backend state. Connection checks
    /// (`extension_connected`, `cdp_available`, `is_connected`) live on the returned
    /// executor so the status the caller reports and the backend it actually drives can
    /// never disagree. Do not re-expose those checks on `Manager` — a second facade over
    /// the same `bridge`/`cdp` state is what lets the two drift and hide bugs.
    pub fn executor(&self) -> Option<ActionExecutor> {
        Some(ActionExecutor::new(self.bridge.clone(), self.cdp.clone()))
    }

    /// Launch a managed Chrome instance for a profile.
    pub async fn launch(&self, profile_name: &str) -> Result<(), BrowserError> {
        let profile = self
            .config
            .resolve_profile(profile_name, &self.data_dir)
            .ok_or_else(|| BrowserError::Other(format!("profile {} not found", profile_name)))?;

        if profile.driver != "nebo" {
            return Err(BrowserError::Other(
                "only 'nebo' driver profiles can be launched".into(),
            ));
        }

        // Check if already running
        {
            let browsers = self.browsers.read().await;
            if browsers.contains_key(profile_name) {
                return Ok(());
            }
        }

        let chrome = RunningChrome::launch(
            self.config.executable_path.as_deref(),
            &profile.user_data_dir,
            profile.cdp_port,
            self.config.headless,
            self.config.no_sandbox,
        )
        .await?;

        let cdp_url = format!("http://127.0.0.1:{}", profile.cdp_port);
        let session = Arc::new(Session::new(profile_name, &cdp_url));

        let mut browsers = self.browsers.write().await;
        browsers.insert(profile_name.to_string(), chrome);

        let mut sessions = self.sessions.write().await;
        sessions.insert(profile_name.to_string(), session);

        info!(profile = profile_name, "browser launched");
        Ok(())
    }

    /// Stop a managed Chrome instance.
    pub async fn stop(&self, profile_name: &str) -> Result<(), BrowserError> {
        let mut browsers = self.browsers.write().await;
        if let Some(mut chrome) = browsers.remove(profile_name) {
            chrome.kill().await;
        }

        let mut sessions = self.sessions.write().await;
        sessions.remove(profile_name);

        info!(profile = profile_name, "browser stopped");
        Ok(())
    }

    /// Get or create a session for a profile.
    pub async fn get_or_create_session(
        &self,
        profile_name: &str,
    ) -> Result<Arc<Session>, BrowserError> {
        // Check existing
        {
            let sessions = self.sessions.read().await;
            if let Some(session) = sessions.get(profile_name) {
                return Ok(session.clone());
            }
        }

        let profile = self
            .config
            .resolve_profile(profile_name, &self.data_dir)
            .ok_or_else(|| BrowserError::Other(format!("profile {} not found", profile_name)))?;

        // For managed profiles, launch Chrome first
        if profile.driver == "nebo" {
            self.launch(profile_name).await?;
            let sessions = self.sessions.read().await;
            return sessions
                .get(profile_name)
                .cloned()
                .ok_or_else(|| BrowserError::SessionNotFound(profile_name.into()));
        }

        // For extension profiles, create session pointing at configured CDP URL
        let cdp_url = profile
            .cdp_url
            .unwrap_or_else(|| format!("http://127.0.0.1:{}", profile.cdp_port));
        let session = Arc::new(Session::new(profile_name, &cdp_url));
        let mut sessions = self.sessions.write().await;
        sessions.insert(profile_name.to_string(), session.clone());
        Ok(session)
    }

    /// Get session if it exists.
    pub async fn get_session(&self, profile_name: &str) -> Option<Arc<Session>> {
        self.sessions.read().await.get(profile_name).cloned()
    }

    /// List profile statuses.
    pub async fn list_profiles(&self) -> Vec<ProfileStatus> {
        let browsers = self.browsers.read().await;
        let sessions = self.sessions.read().await;

        self.config
            .profiles
            .iter()
            .map(|(name, cfg)| {
                let running = browsers.contains_key(name);
                let page_count = sessions.get(name).map(|s| s.page_count()).unwrap_or(0);
                ProfileStatus {
                    name: name.clone(),
                    driver: cfg.driver.clone(),
                    running,
                    page_count,
                    cdp_port: cfg.cdp_port.unwrap_or(9222),
                }
            })
            .collect()
    }

    /// Shutdown all browsers.
    pub async fn shutdown(&self) {
        let mut browsers = self.browsers.write().await;
        for (name, mut chrome) in browsers.drain() {
            chrome.kill().await;
            info!(profile = name.as_str(), "browser shutdown");
        }
        let mut sessions = self.sessions.write().await;
        sessions.clear();
        // The built-in CDP Chrome (if launched) is killed when its RunningChrome drops.
    }
}
