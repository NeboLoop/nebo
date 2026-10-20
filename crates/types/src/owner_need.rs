//! What only the owner can supply before an employee's duty can run, as
//! data. The code that meets the wall names it (a plugin tool refusing for
//! want of an account knows the plugin); everything downstream reads this,
//! never the refusal's words.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OwnerNeed {
    /// Some plugin that provides the capability (`telephony`, `mail`) must
    /// be installed: none installed here does.
    Capability { capability: String },
    /// The plugin must be installed or turned on.
    Plugin { plugin: String },
    /// The plugin is installed; an account on it must be connected for
    /// this employee.
    Account { plugin: String },
}

impl OwnerNeed {
    /// A stable key for "the owner was told this": `capability:<name>`,
    /// `account:<plugin>`, `plugin:<plugin>`.
    pub fn key(&self) -> String {
        match self {
            OwnerNeed::Capability { capability } => format!("capability:{capability}"),
            OwnerNeed::Plugin { plugin } => format!("plugin:{plugin}"),
            OwnerNeed::Account { plugin } => format!("account:{plugin}"),
        }
    }

    /// The need a [`OwnerNeed::key`] names, or None for any other key.
    pub fn from_key(key: &str) -> Option<Self> {
        let (kind, name) = key.split_once(':')?;
        let name = name.to_string();
        match kind {
            "capability" => Some(OwnerNeed::Capability { capability: name }),
            "plugin" => Some(OwnerNeed::Plugin { plugin: name }),
            "account" => Some(OwnerNeed::Account { plugin: name }),
            _ => None,
        }
    }
}

/// A decision only the owner can make, asked by a run that can't reach them
/// (a helper): it travels back to the conversation that started the run,
/// which asks the owner and sends the answer on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerQuestion {
    pub question: String,
    /// What the owner decides on: a draft, a plan, what was found.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub context: String,
    /// The choices, the recommended one first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

impl OwnerQuestion {
    /// The recommended choice: the first option.
    pub fn recommended(&self) -> Option<&str> {
        self.options.first().map(String::as_str)
    }
}
