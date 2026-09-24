//! What only the owner can supply before an employee's duty can run, as
//! data. The code that meets the wall names it (a plugin tool refusing for
//! want of an account knows the plugin); everything downstream reads this,
//! never the refusal's words.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OwnerNeed {
    /// The plugin must be installed or turned on.
    Plugin { plugin: String },
    /// The plugin is installed; an account on it must be connected for
    /// this employee.
    Account { plugin: String },
}

impl OwnerNeed {
    /// A stable key for "the owner was told this": `account:<plugin>`,
    /// `plugin:<plugin>`.
    pub fn key(&self) -> String {
        match self {
            OwnerNeed::Plugin { plugin } => format!("plugin:{plugin}"),
            OwnerNeed::Account { plugin } => format!("account:{plugin}"),
        }
    }
}
