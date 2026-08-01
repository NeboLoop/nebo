//! Migration importer — adopt an existing Hermes / OpenClaw install into Nebo.
//!
//! Two halves: the **dry-run** — [`detect`] identifies which system an install
//! directory belongs to and [`scan`] walks it read-only into an
//! [`ImportManifest`] describing what was found and what each piece becomes in
//! Nebo — and the **apply** — [`apply`] turns that install into real Nebo
//! artifacts (MCP integrations, skills, the employee persona, provider keys),
//! idempotently and without ever writing to the source directory. Memory,
//! history, cron, and channel tokens are later slices, reported as skipped.

mod apply;
mod detect;
mod hermes;
mod manifest;
mod openclaw;
mod parse;

pub use apply::{apply, apply_hermes, apply_openclaw, ApplyTargets, ImportOutcome};
pub use detect::detect;
pub use manifest::{ImportItem, ImportManifest, ItemKind, SourceKind, TrustTier};

use std::path::Path;

use types::NeboError;

/// Detect the agent system at `root` and build its dry-run manifest.
///
/// Read-only: never writes to Nebo, never modifies `root`. Returns
/// [`NeboError::Validation`] if the directory isn't a recognized install.
pub fn scan(root: &Path) -> Result<ImportManifest, NeboError> {
    match detect(root) {
        Some(SourceKind::Hermes) => Ok(hermes::scan(root)),
        Some(SourceKind::OpenClaw) => Ok(openclaw::scan(root)),
        None => Err(NeboError::Validation(format!(
            "{} is not a recognized Hermes or OpenClaw install directory",
            root.display()
        ))),
    }
}
