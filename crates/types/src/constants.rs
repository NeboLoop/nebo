/// Default HTTP server port.
pub const DEFAULT_PORT: u16 = 27895;

/// Default bind host.
pub const DEFAULT_HOST: &str = "127.0.0.1";

/// Default access token expiry in seconds (1 year).
pub const DEFAULT_ACCESS_EXPIRE: i64 = 31_536_000;

/// Default refresh token expiry in seconds (1 year).
pub const DEFAULT_REFRESH_TOKEN_EXPIRE: i64 = 31_536_000;

/// Default CSRF token expiry in seconds (12 hours).
pub const DEFAULT_CSRF_TOKEN_EXPIRY: i64 = 43_200;

/// Default API rate limit (requests per interval).
pub const DEFAULT_RATE_LIMIT_REQUESTS: u32 = 100;

/// Default API rate limit interval in seconds.
pub const DEFAULT_RATE_LIMIT_INTERVAL: u32 = 60;

/// Default API rate limit burst.
pub const DEFAULT_RATE_LIMIT_BURST: u32 = 20;

/// Default auth rate limit (requests per interval).
pub const DEFAULT_AUTH_RATE_LIMIT_REQUESTS: u32 = 5;

/// Default auth rate limit interval in seconds.
pub const DEFAULT_AUTH_RATE_LIMIT_INTERVAL: u32 = 60;

/// Default max request body size (10 MB).
pub const DEFAULT_MAX_REQUEST_BODY_SIZE: i64 = 10_485_760;

/// Default max URL length.
pub const DEFAULT_MAX_URL_LENGTH: u32 = 2048;

/// Default SMTP port.
pub const DEFAULT_SMTP_PORT: u16 = 587;

/// Lane names for the agent concurrency system.
pub mod lanes {
    pub const MAIN: &str = "main";
    pub const EVENTS: &str = "events";
    pub const SUBAGENT: &str = "subagent";
    pub const NESTED: &str = "nested";
    pub const HEARTBEAT: &str = "heartbeat";
    pub const COMM: &str = "comm";
    pub const DEV: &str = "dev";
    pub const DESKTOP: &str = "desktop";
}

/// Origin types for tool restrictions.
pub mod origins {
    pub const USER: &str = "user";
    pub const COMM: &str = "comm";
    pub const APP: &str = "app";
    pub const SKILL: &str = "skill";
    pub const SYSTEM: &str = "system";
}

/// File names used in the data directory.
pub mod files {
    pub const SETTINGS_JSON: &str = "settings.json";
    pub const BOT_ID: &str = "bot_id";
    pub const SETUP_COMPLETE: &str = ".setup-complete";
    pub const DATABASE: &str = "data/nebo.db";
    pub const MODELS_YAML: &str = "models.yaml";
    pub const CONFIG_YAML: &str = "config.yaml";
}
