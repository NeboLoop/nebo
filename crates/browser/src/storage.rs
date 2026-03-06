use serde::{Deserialize, Serialize};

use crate::Cookie;

/// Storage kind: localStorage or sessionStorage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageKind {
    Local,
    Session,
}

impl StorageKind {
    /// Get the JavaScript storage object name.
    pub fn js_name(&self) -> &str {
        match self {
            StorageKind::Local => "localStorage",
            StorageKind::Session => "sessionStorage",
        }
    }
}

impl std::fmt::Display for StorageKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageKind::Local => write!(f, "local"),
            StorageKind::Session => write!(f, "session"),
        }
    }
}

/// Saved storage state for session persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageState {
    pub cookies: Vec<Cookie>,
    #[serde(default)]
    pub local_storage: Vec<StorageEntry>,
    #[serde(default)]
    pub session_storage: Vec<StorageEntry>,
}

/// A key-value entry in web storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageEntry {
    pub origin: String,
    pub key: String,
    pub value: String,
}

/// JavaScript snippet to get all keys/values from a storage kind.
pub fn js_get_all_storage(kind: StorageKind) -> String {
    format!(
        r#"
        (() => {{
            const s = window.{};
            const result = {{}};
            for (let i = 0; i < s.length; i++) {{
                const key = s.key(i);
                result[key] = s.getItem(key);
            }}
            return JSON.stringify(result);
        }})()
        "#,
        kind.js_name()
    )
}

/// JavaScript snippet to get a single key from storage.
pub fn js_get_storage(kind: StorageKind, key: &str) -> String {
    format!("window.{}.getItem('{}')", kind.js_name(), key.replace('\'', "\\'"))
}

/// JavaScript snippet to set a key in storage.
pub fn js_set_storage(kind: StorageKind, key: &str, value: &str) -> String {
    format!(
        "window.{}.setItem('{}', '{}')",
        kind.js_name(),
        key.replace('\'', "\\'"),
        value.replace('\'', "\\'")
    )
}

/// JavaScript snippet to remove a key from storage.
pub fn js_remove_storage(kind: StorageKind, key: &str) -> String {
    format!("window.{}.removeItem('{}')", kind.js_name(), key.replace('\'', "\\'"))
}

/// JavaScript snippet to clear all storage.
pub fn js_clear_storage(kind: StorageKind) -> String {
    format!("window.{}.clear()", kind.js_name())
}
