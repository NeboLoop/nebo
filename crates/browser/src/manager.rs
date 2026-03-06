use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::info;

use crate::chrome::RunningChrome;
use crate::config::BrowserConfig;
use crate::executor::ActionExecutor;
use crate::extension_bridge::ExtensionBridge;
use crate::session::Session;
use crate::BrowserError;

/// Manages browser instances and sessions.
pub struct Manager {
    config: BrowserConfig,
    data_dir: String,
    browsers: RwLock<HashMap<String, RunningChrome>>,
    sessions: RwLock<HashMap<String, Arc<Session>>>,
    bridge: Arc<ExtensionBridge>,
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
        Self {
            config,
            data_dir,
            browsers: RwLock::new(HashMap::new()),
            sessions: RwLock::new(HashMap::new()),
            bridge: Arc::new(ExtensionBridge::new()),
        }
    }

    /// Get the extension bridge for WS handler wiring.
    pub fn bridge(&self) -> Arc<ExtensionBridge> {
        self.bridge.clone()
    }

    /// Get an ActionExecutor for extension-based browser automation.
    pub fn executor(&self) -> Option<ActionExecutor> {
        Some(ActionExecutor::new(self.bridge.clone()))
    }

    /// Check if the Chrome extension is connected via the bridge.
    pub fn extension_connected(&self) -> bool {
        self.bridge.is_connected()
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
                let page_count = sessions
                    .get(name)
                    .map(|s| s.page_count())
                    .unwrap_or(0);
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
    }
}
