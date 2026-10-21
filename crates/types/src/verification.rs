//! What the owner reads about a piece of verified code work or one of its
//! steps: a code the apps word in the owner's language, with its values.

use std::collections::BTreeMap;

/// A code and its values. The record keeps it as JSON (a verification's
/// `reason`, a round's `outcome`); the apps word it, and the raw text
/// behind it (a git error, a reviewer's failure) stays in the step's detail
/// and the log, never on the owner's screen.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Why {
    pub code: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub args: BTreeMap<String, serde_json::Value>,
}

impl Why {
    pub fn new(code: &str) -> Self {
        Self { code: code.to_string(), args: Default::default() }
    }

    pub fn with(mut self, key: &str, value: impl Into<serde_json::Value>) -> Self {
        self.args.insert(key.to_string(), value.into());
        self
    }

    /// As the record keeps it.
    pub fn text(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Read back from the record; `None` for nothing (or an old row's words).
    pub fn read(text: &str) -> Option<Self> {
        serde_json::from_str::<Self>(text).ok().filter(|w| !w.code.is_empty())
    }
}
