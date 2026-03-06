use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Browser automation configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_control_port")]
    pub control_port: u16,
    #[serde(default)]
    pub executable_path: Option<String>,
    #[serde(default)]
    pub headless: bool,
    #[serde(default)]
    pub no_sandbox: bool,
    #[serde(default)]
    pub profiles: HashMap<String, ProfileConfig>,
}

fn default_control_port() -> u16 {
    9223
}

impl Default for BrowserConfig {
    fn default() -> Self {
        let mut profiles = HashMap::new();
        profiles.insert(
            "nebo".to_string(),
            ProfileConfig {
                driver: "nebo".to_string(),
                cdp_port: Some(9222),
                cdp_url: None,
                color: Some("#6366f1".to_string()),
            },
        );
        profiles.insert(
            "chrome".to_string(),
            ProfileConfig {
                driver: "extension".to_string(),
                cdp_port: None,
                cdp_url: None,
                color: None,
            },
        );

        Self {
            enabled: false,
            control_port: 9223,
            executable_path: None,
            headless: false,
            no_sandbox: false,
            profiles,
        }
    }
}

/// Per-profile configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileConfig {
    #[serde(default = "default_driver")]
    pub driver: String,
    #[serde(default)]
    pub cdp_port: Option<u16>,
    #[serde(default)]
    pub cdp_url: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
}

fn default_driver() -> String {
    "nebo".to_string()
}

/// Resolved profile with all defaults applied.
#[derive(Debug, Clone)]
pub struct ResolvedProfile {
    pub name: String,
    pub cdp_port: u16,
    pub cdp_url: Option<String>,
    pub cdp_is_loopback: bool,
    pub driver: String,
    pub color: String,
    pub user_data_dir: String,
}

impl BrowserConfig {
    /// Resolve a profile by name, applying defaults.
    pub fn resolve_profile(&self, name: &str, data_dir: &str) -> Option<ResolvedProfile> {
        let profile = self.profiles.get(name)?;

        let cdp_port = profile.cdp_port.unwrap_or(9222);
        let user_data_dir = format!("{}/browser/{}", data_dir, name);

        Some(ResolvedProfile {
            name: name.to_string(),
            cdp_port,
            cdp_url: profile.cdp_url.clone(),
            cdp_is_loopback: true,
            driver: profile.driver.clone(),
            color: profile.color.clone().unwrap_or_else(|| "#6366f1".to_string()),
            user_data_dir,
        })
    }
}
