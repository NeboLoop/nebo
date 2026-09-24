//! What a tool call is, as the permission check reads it. Each tool's spec
//! (`tools::registry::DynTool`) answers these for one call; the registry
//! resolves them into a [`Target`] from the call as it will run. The check
//! itself (modes, rules, asks) builds on this data.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The value of a call that a rule can match, beside its rule key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum RuleField {
    /// A shell command, matched by prefix.
    CommandPrefix(String),
    /// A file or folder, matched against folder rules.
    Folder(PathBuf),
    /// A web host.
    Domain(String),
    /// Who a message goes to.
    Recipient(String),
}

/// Whether an effect happens, when the call's input can say.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Knowable {
    Yes,
    No,
    /// The input can't tell. Never a guess.
    #[default]
    Unknown,
}

/// What a call does outside its own work, as far as its input shows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallEffects {
    pub money_cents: Option<i64>,
    pub counterparty: Option<String>,
    pub recipients: Vec<String>,
    pub publishes: Knowable,
    pub deletes: Vec<String>,
    pub overwrites: Vec<String>,
}

impl CallEffects {
    /// A call that changes nothing outside this process.
    pub fn none() -> Self {
        Self {
            publishes: Knowable::No,
            ..Self::default()
        }
    }

    /// A call whose outward effects the input can't show.
    pub fn unknown() -> Self {
        Self::default()
    }
}

/// One call, resolved for the permission check from the tool's spec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// The registered tool that runs the call.
    pub tool: String,
    /// The key rules match: a tool name of the current set, or a catalog
    /// operation.
    pub key: String,
    pub operation: Option<String>,
    /// The job capability the call belongs to; `None` is basic work.
    pub capability: Option<String>,
    pub field: Option<RuleField>,
    pub read_only: bool,
    pub effects: CallEffects,
}
