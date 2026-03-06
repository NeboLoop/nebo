use serde::{Deserialize, Serialize};

use crate::session::Page;
use crate::BrowserError;

/// Options for navigation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavigateOptions {
    pub url: String,
    #[serde(default = "default_wait_until")]
    pub wait_until: String,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_wait_until() -> String {
    "domcontentloaded".to_string()
}

fn default_timeout() -> u64 {
    30000
}

/// Options for clicking an element.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClickOptions {
    #[serde(default)]
    pub r#ref: Option<String>,
    #[serde(default)]
    pub selector: Option<String>,
    #[serde(default = "default_button")]
    pub button: String,
    #[serde(default = "default_click_count")]
    pub count: u32,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_button() -> String {
    "left".to_string()
}

fn default_click_count() -> u32 {
    1
}

/// Options for typing text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeOptions {
    #[serde(default)]
    pub r#ref: Option<String>,
    #[serde(default)]
    pub selector: Option<String>,
    pub text: String,
    #[serde(default)]
    pub delay_ms: u64,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

/// Options for filling an input (replaces value).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FillOptions {
    #[serde(default)]
    pub r#ref: Option<String>,
    #[serde(default)]
    pub selector: Option<String>,
    pub value: String,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

/// Options for selecting from a dropdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectOptions {
    #[serde(default)]
    pub r#ref: Option<String>,
    #[serde(default)]
    pub selector: Option<String>,
    pub value: String,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

/// Options for hovering over an element.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoverOptions {
    #[serde(default)]
    pub r#ref: Option<String>,
    #[serde(default)]
    pub selector: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

/// Options for pressing a key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PressOptions {
    pub key: String,
    #[serde(default)]
    pub r#ref: Option<String>,
    #[serde(default)]
    pub selector: Option<String>,
}

/// Options for scrolling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrollOptions {
    #[serde(default = "default_direction")]
    pub direction: String,
    #[serde(default = "default_amount")]
    pub amount: i32,
}

fn default_direction() -> String {
    "down".to_string()
}

fn default_amount() -> i32 {
    300
}

/// Options for waiting for an element.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaitOptions {
    #[serde(default)]
    pub r#ref: Option<String>,
    #[serde(default)]
    pub selector: Option<String>,
    #[serde(default = "default_state")]
    pub state: String,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_state() -> String {
    "visible".to_string()
}

/// Options for taking a screenshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenshotOptions {
    #[serde(default)]
    pub r#ref: Option<String>,
    #[serde(default)]
    pub selector: Option<String>,
    #[serde(default)]
    pub full_page: bool,
}

/// Resolve the target selector from options (ref takes precedence).
pub fn resolve_target(page: &Page, r#ref: &Option<String>, selector: &Option<String>) -> Result<String, BrowserError> {
    if let Some(r) = r#ref {
        return Ok(page.resolve_selector(r));
    }
    if let Some(s) = selector {
        return Ok(s.clone());
    }
    Err(BrowserError::ElementNotFound("no ref or selector provided".into()))
}

/// Interactive roles that get element refs in accessibility snapshots.
pub const INTERACTIVE_ROLES: &[&str] = &[
    "button", "link", "textbox", "checkbox", "radio", "combobox",
    "listbox", "menuitem", "tab", "slider", "spinbutton", "switch",
    "searchbox", "textarea",
];
