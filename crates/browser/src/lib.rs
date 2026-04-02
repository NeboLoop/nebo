pub mod actions;
pub mod audit;
pub mod chrome;
pub mod config;
pub mod executor;
pub mod extension_bridge;
pub mod manager;
pub mod native_host;
pub mod native_types;
pub mod session;
pub mod snapshot;
pub mod snapshot_store;
pub mod storage;

pub use config::{BrowserConfig, ProfileConfig, ResolvedProfile};
pub use executor::ActionExecutor;
pub use extension_bridge::{BatchAction, BatchOptions, ExtensionBridge};
pub use manager::Manager;
pub use native_host::NativeHost;
pub use session::{Page, PageState, Session};
pub use snapshot_store::SnapshotStore;

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum BrowserError {
    #[error("Chrome not found")]
    ChromeNotFound,
    #[error("CDP connection failed: {0}")]
    CdpConnection(String),
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("element not found: {0}")]
    ElementNotFound(String),
    #[error("session not found: {0}")]
    SessionNotFound(String),
    #[error("page not found: {0}")]
    PageNotFound(String),
    #[error("Chrome extension not connected")]
    ExtensionNotConnected,
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Other(String),
}

/// Element reference for accessibility tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementRef {
    pub id: String,       // e.g. "e1", "e2"
    pub role: String,     // button, link, textbox, etc.
    pub name: String,     // accessible name
    pub selector: String, // CSS or role-based selector
}

/// Console message captured from a page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsoleMessage {
    pub level: String,
    pub text: String,
    pub timestamp: i64,
}

/// Page error captured from a page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageError {
    pub message: String,
    pub timestamp: i64,
}

/// Cookie representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub expires: f64,
    #[serde(default)]
    pub http_only: bool,
    #[serde(default)]
    pub secure: bool,
    #[serde(default)]
    pub same_site: String,
}
