use std::collections::HashMap;
use std::env;

use serde::Deserialize;

use crate::defaults;
use types::constants::*;

/// Top-level Nebo configuration, loaded from YAML with env var expansion.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Host")]
    pub host: String,
    #[serde(rename = "Port")]
    pub port: u16,
    #[serde(rename = "Timeout")]
    pub timeout: u64,
    #[serde(rename = "App")]
    pub app: AppConfig,
    #[serde(rename = "Auth")]
    pub auth: AuthConfig,
    #[serde(rename = "Database")]
    pub database: DatabaseConfig,
    #[serde(rename = "Security")]
    pub security: SecurityConfig,
    #[serde(rename = "Email")]
    pub email: EmailConfig,
    #[serde(rename = "OAuth")]
    pub oauth: OAuthConfig,
    #[serde(rename = "Features")]
    pub features: FeaturesConfig,
    #[serde(rename = "NeboLoop")]
    pub neboloop: NeboLoopConfig,
    #[serde(rename = "AppOAuth")]
    pub app_oauth: HashMap<String, AppOAuthProviderConfig>,
    #[serde(rename = "Log")]
    pub log: LogConfig,
    /// Local Chrome extension ID for development (load unpacked).
    /// Production Web Store ID is always included automatically.
    #[serde(rename = "BrowserExtensionId", default)]
    pub browser_extension_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    #[serde(rename = "BaseURL")]
    pub base_url: String,
    #[serde(rename = "Domain")]
    pub domain: String,
    #[serde(rename = "ProductionMode")]
    pub production_mode: String,
    #[serde(rename = "AdminEmail")]
    pub admin_email: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    #[serde(rename = "AccessSecret")]
    pub access_secret: String,
    #[serde(rename = "AccessExpire")]
    pub access_expire: i64,
    #[serde(rename = "RefreshTokenExpire")]
    pub refresh_token_expire: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    #[serde(rename = "SQLitePath")]
    pub sqlite_path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    #[serde(rename = "CSRFEnabled")]
    pub csrf_enabled: String,
    #[serde(rename = "CSRFSecret")]
    pub csrf_secret: String,
    #[serde(rename = "CSRFTokenExpiry")]
    pub csrf_token_expiry: i64,
    #[serde(rename = "CSRFSecureCookie")]
    pub csrf_secure_cookie: String,
    #[serde(rename = "RateLimitEnabled")]
    pub rate_limit_enabled: String,
    #[serde(rename = "RateLimitRequests")]
    pub rate_limit_requests: u32,
    #[serde(rename = "RateLimitInterval")]
    pub rate_limit_interval: u32,
    #[serde(rename = "RateLimitBurst")]
    pub rate_limit_burst: u32,
    #[serde(rename = "AuthRateLimitRequests")]
    pub auth_rate_limit_requests: u32,
    #[serde(rename = "AuthRateLimitInterval")]
    pub auth_rate_limit_interval: u32,
    #[serde(rename = "EnableSecurityHeaders")]
    pub enable_security_headers: String,
    #[serde(rename = "ContentSecurityPolicy")]
    pub content_security_policy: String,
    #[serde(rename = "AllowedOrigins")]
    pub allowed_origins: String,
    #[serde(rename = "ForceHTTPS")]
    pub force_https: String,
    #[serde(rename = "MaxRequestBodySize")]
    pub max_request_body_size: i64,
    #[serde(rename = "MaxURLLength")]
    pub max_url_length: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct EmailConfig {
    #[serde(rename = "SMTPHost")]
    pub smtp_host: String,
    #[serde(rename = "SMTPPort")]
    pub smtp_port: u16,
    #[serde(rename = "SMTPUser")]
    pub smtp_user: String,
    #[serde(rename = "SMTPPass")]
    pub smtp_pass: String,
    #[serde(rename = "FromAddress")]
    pub from_address: String,
    #[serde(rename = "FromName")]
    pub from_name: String,
    #[serde(rename = "ReplyTo")]
    pub reply_to: String,
    #[serde(rename = "BaseURL")]
    pub base_url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct OAuthConfig {
    #[serde(rename = "GoogleEnabled")]
    pub google_enabled: String,
    #[serde(rename = "GoogleClientID")]
    pub google_client_id: String,
    #[serde(rename = "GoogleClientSecret")]
    pub google_client_secret: String,
    #[serde(rename = "GitHubEnabled")]
    pub github_enabled: String,
    #[serde(rename = "GitHubClientID")]
    pub github_client_id: String,
    #[serde(rename = "GitHubClientSecret")]
    pub github_client_secret: String,
    #[serde(rename = "CallbackBaseURL")]
    pub callback_base_url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FeaturesConfig {
    #[serde(rename = "NotificationsEnabled")]
    pub notifications_enabled: String,
    #[serde(rename = "OAuthEnabled")]
    pub oauth_enabled: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct NeboLoopConfig {
    #[serde(rename = "Enabled")]
    pub enabled: String,
    #[serde(rename = "ApiURL")]
    pub api_url: String,
    #[serde(rename = "JanusURL")]
    pub janus_url: String,
    #[serde(rename = "CommsURL")]
    pub comms_url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct AppOAuthProviderConfig {
    #[serde(rename = "ClientID")]
    pub client_id: String,
    #[serde(rename = "ClientSecret")]
    pub client_secret: String,
    #[serde(rename = "TenantID")]
    pub tenant_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LogConfig {
    #[serde(rename = "Mode")]
    pub mode: String,
    #[serde(rename = "Encoding")]
    pub encoding: String,
    #[serde(rename = "Level")]
    pub level: String,
    #[serde(rename = "Stat")]
    pub stat: bool,
}

// --- Default implementations ---

impl Default for Config {
    fn default() -> Self {
        Self {
            name: "nebo".into(),
            host: DEFAULT_HOST.into(),
            port: DEFAULT_PORT,
            timeout: 60_000,
            app: AppConfig::default(),
            auth: AuthConfig::default(),
            database: DatabaseConfig::default(),
            security: SecurityConfig::default(),
            email: EmailConfig::default(),
            oauth: OAuthConfig::default(),
            features: FeaturesConfig::default(),
            neboloop: NeboLoopConfig::default(),
            app_oauth: HashMap::new(),
            log: LogConfig::default(),
            browser_extension_id: None,
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            base_url: format!("http://localhost:{DEFAULT_PORT}"),
            domain: "localhost".into(),
            production_mode: String::new(),
            admin_email: String::new(),
        }
    }
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            access_secret: "placeholder-replaced-at-runtime".into(),
            access_expire: DEFAULT_ACCESS_EXPIRE,
            refresh_token_expire: DEFAULT_REFRESH_TOKEN_EXPIRE,
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        let sqlite_path = defaults::data_dir()
            .map(|d| {
                let mut p = d;
                p.push("data");
                p.push("nebo.db");
                p.to_string_lossy().into_owned()
            })
            .unwrap_or_default();
        Self { sqlite_path }
    }
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            csrf_enabled: "true".into(),
            csrf_secret: String::new(),
            csrf_token_expiry: DEFAULT_CSRF_TOKEN_EXPIRY,
            csrf_secure_cookie: "true".into(),
            rate_limit_enabled: "true".into(),
            rate_limit_requests: DEFAULT_RATE_LIMIT_REQUESTS,
            rate_limit_interval: DEFAULT_RATE_LIMIT_INTERVAL,
            rate_limit_burst: DEFAULT_RATE_LIMIT_BURST,
            auth_rate_limit_requests: DEFAULT_AUTH_RATE_LIMIT_REQUESTS,
            auth_rate_limit_interval: DEFAULT_AUTH_RATE_LIMIT_INTERVAL,
            enable_security_headers: "true".into(),
            content_security_policy: String::new(),
            allowed_origins: String::new(),
            force_https: String::new(),
            max_request_body_size: DEFAULT_MAX_REQUEST_BODY_SIZE,
            max_url_length: DEFAULT_MAX_URL_LENGTH,
        }
    }
}

impl Default for EmailConfig {
    fn default() -> Self {
        Self {
            smtp_host: String::new(),
            smtp_port: DEFAULT_SMTP_PORT,
            smtp_user: String::new(),
            smtp_pass: String::new(),
            from_address: String::new(),
            from_name: "nebo".into(),
            reply_to: String::new(),
            base_url: "http://localhost:27458".into(),
        }
    }
}


impl Default for FeaturesConfig {
    fn default() -> Self {
        Self {
            notifications_enabled: "true".into(),
            oauth_enabled: String::new(),
        }
    }
}

impl Default for NeboLoopConfig {
    fn default() -> Self {
        Self {
            enabled: "true".into(),
            api_url: "https://api.neboloop.com".into(),
            janus_url: "https://janus.neboloop.com".into(),
            comms_url: "wss://comms.neboloop.com/ws".into(),
        }
    }
}


impl Default for LogConfig {
    fn default() -> Self {
        Self {
            mode: "console".into(),
            encoding: "plain".into(),
            level: "info".into(),
            stat: false,
        }
    }
}

// --- Config methods ---

impl Config {
    /// Load configuration from YAML bytes with environment variable expansion.
    pub fn load_from_bytes(data: &[u8]) -> Result<Self, types::NeboError> {
        let text = String::from_utf8_lossy(data);
        let expanded = shellexpand::env(&text)
            .map_err(|e| types::NeboError::Config(format!("env expansion: {e}")))?;
        let mut config: Config = serde_yaml::from_str(&expanded)
            .map_err(|e| types::NeboError::Config(format!("yaml parse: {e}")))?;
        config.apply_defaults();
        Ok(config)
    }

    /// Load the embedded default configuration.
    pub fn load_embedded() -> Result<Self, types::NeboError> {
        Self::load_from_bytes(include_bytes!("../../../etc/nebo.yaml"))
    }

    fn apply_defaults(&mut self) {
        if self.host.is_empty() {
            self.host = DEFAULT_HOST.into();
        }
        if self.port == 0 {
            self.port = DEFAULT_PORT;
        }
        if self.app.domain.is_empty() {
            self.app.domain = "localhost".into();
        }
        if self.app.base_url.is_empty() {
            self.app.base_url = format!("http://localhost:{}", self.port);
        }
        if self.auth.refresh_token_expire == 0 {
            self.auth.refresh_token_expire = DEFAULT_REFRESH_TOKEN_EXPIRE;
        }
        if self.database.sqlite_path.is_empty()
            && let Ok(dir) = defaults::data_dir() {
                self.database.sqlite_path =
                    dir.join("data").join("nebo.db").to_string_lossy().into_owned();
            }
        // Apply NeboLoop env var overrides
        if let Ok(v) = env::var("NEBOLOOP_API_URL") {
            self.neboloop.api_url = v;
        }
        if let Ok(v) = env::var("NEBOLOOP_JANUS_URL") {
            self.neboloop.janus_url = v;
        }
        if let Ok(v) = env::var("NEBOLOOP_COMMS_URL") {
            self.neboloop.comms_url = v;
        }
    }

    pub fn is_production_mode(&self) -> bool {
        parse_bool(&self.app.production_mode, false)
    }
    pub fn is_csrf_enabled(&self) -> bool {
        parse_bool(&self.security.csrf_enabled, true)
    }
    pub fn is_rate_limit_enabled(&self) -> bool {
        parse_bool(&self.security.rate_limit_enabled, true)
    }
    pub fn is_security_headers_enabled(&self) -> bool {
        parse_bool(&self.security.enable_security_headers, true)
    }
    pub fn is_force_https(&self) -> bool {
        parse_bool(&self.security.force_https, false)
    }
    pub fn is_google_oauth_enabled(&self) -> bool {
        parse_bool(&self.oauth.google_enabled, false)
    }
    pub fn is_github_oauth_enabled(&self) -> bool {
        parse_bool(&self.oauth.github_enabled, false)
    }
    pub fn is_notifications_enabled(&self) -> bool {
        parse_bool(&self.features.notifications_enabled, true)
    }
    pub fn is_oauth_enabled(&self) -> bool {
        parse_bool(&self.features.oauth_enabled, false)
    }
    pub fn is_neboloop_enabled(&self) -> bool {
        parse_bool(&self.neboloop.enabled, true)
    }
}

/// Parse a string as boolean, matching Go's parseBool behavior.
fn parse_bool(s: &str, default: bool) -> bool {
    let s = s.trim().to_lowercase();
    if s.is_empty() {
        return default;
    }
    matches!(s.as_str(), "true" | "1" | "yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bool() {
        assert!(parse_bool("true", false));
        assert!(parse_bool("1", false));
        assert!(parse_bool("yes", false));
        assert!(!parse_bool("false", true));
        assert!(!parse_bool("no", true));
        assert!(parse_bool("", true)); // empty uses default
        assert!(!parse_bool("", false));
    }

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.port, DEFAULT_PORT);
        assert_eq!(config.host, DEFAULT_HOST);
        assert!(config.is_csrf_enabled());
        assert!(config.is_rate_limit_enabled());
    }
}
