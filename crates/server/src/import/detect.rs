//! Fingerprint an install directory as a known agent system.

use std::path::Path;

use super::manifest::SourceKind;

/// Identify the agent system at `root`, or `None` if it isn't one we import.
///
/// Cheap and read-only: it checks for signature files, never opens them. The
/// caller is expected to resolve env overrides (`HERMES_HOME`, `OPENCLAW_HOME`,
/// `OPENCLAW_STATE_DIR`) into `root` before calling.
pub fn detect(root: &Path) -> Option<SourceKind> {
    // Hermes: `config.yaml` alongside at least one of its persona / memory /
    // history markers, so a stray `config.yaml` elsewhere isn't misread.
    if root.join("config.yaml").is_file()
        && (root.join("SOUL.md").is_file()
            || root.join("state.db").is_file()
            || root.join("memories").is_dir())
    {
        return Some(SourceKind::Hermes);
    }

    // OpenClaw: the JSON5 config plus its agents / workspace tree.
    if root.join("openclaw.json").is_file()
        && (root.join("agents").is_dir() || root.join("workspace").is_dir())
    {
        return Some(SourceKind::OpenClaw);
    }

    None
}
