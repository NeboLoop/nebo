//! Migration importer — adopt an existing Hermes / OpenClaw install into Nebo.
//!
//! This slice is the **dry-run**: [`detect`] identifies which system an install
//! directory belongs to, and [`scan`] walks it read-only into an
//! [`ImportManifest`] describing what was found and what each piece becomes in
//! Nebo. Nothing here writes to Nebo or modifies the source directory — applying
//! a manifest (creating employees, skills, memory, MCP integrations, and copying
//! credentials) is the next slice.

mod detect;
mod hermes;
mod manifest;

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
        Some(SourceKind::OpenClaw) => Err(NeboError::Validation(
            "OpenClaw install detected — the OpenClaw walker lands in the next slice; \
             Hermes import is available now"
                .into(),
        )),
        None => Err(NeboError::Validation(format!(
            "{} is not a recognized Hermes or OpenClaw install directory",
            root.display()
        ))),
    }
}
