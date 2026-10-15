//! The dry-run data model: what an import *would* do, before it does anything.
//!
//! A manifest is produced by walking a foreign install read-only. It is the
//! single thing the "point Nebo at an install" UX renders and the user approves.

use serde::Serialize;

use crate::handlers::integrations::ParsedMcpServer;

/// Which foreign agent system an install directory belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Hermes,
    OpenClaw,
}

impl SourceKind {
    pub fn label(self) -> &'static str {
        match self {
            SourceKind::Hermes => "Hermes",
            SourceKind::OpenClaw => "OpenClaw",
        }
    }
}

/// The Nebo artifact an imported item maps onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    McpServer,
    Skill,
    Agent,
    Memory,
    Session,
    Cron,
    Credential,
}

/// Execution risk of adopting an item — drives how much confirmation it needs.
///
/// Mirrors the paste-import trust tiers so the two surfaces stay consistent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustTier {
    /// Pure content — safe to adopt without a prompt (remote MCP, a markdown
    /// skill, a persona file, memory, a schedule).
    Content,
    /// Runs code on the machine — a stdio MCP server that launches a
    /// subprocess, or a skill that bundles executable scripts.
    Code,
    /// Points off-machine at a payload not shown at scan time (a bare URL or
    /// install reference). Reserved for later walkers.
    Reference,
}

/// One thing found in a foreign install, and what it becomes in Nebo.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportItem {
    pub kind: ItemKind,
    pub tier: TrustTier,
    /// Human-facing name: server name, skill name, file stem, or secret key.
    pub name: String,
    /// One-line specifics for the manifest. Never contains a secret *value*.
    pub detail: String,
    /// What this becomes in Nebo, e.g. "Connector", "Nebo skill".
    pub target: &'static str,
    /// Path relative to the install root, for provenance in the dry-run.
    pub source_path: String,
}

impl ImportItem {
    /// A connector from an install's MCP block, as both walkers report it.
    /// One that runs on this device (stdio) is Code tier, and its detail is
    /// the exact command line it starts, so the owner reviews what will run
    /// before importing (as Claude Desktop's config review shows it). Its
    /// environment is named, never its values: they are often keys. A
    /// remote one is Content tier: its transport and address.
    pub(super) fn connector(s: ParsedMcpServer, source_path: &str) -> Self {
        let local = s.server_type == "stdio";
        let detail = if local {
            let spec = s.metadata.as_deref().and_then(|md| serde_json::from_str::<serde_json::Value>(md).ok());
            spec.as_ref().map(command_line).unwrap_or_else(|| "a program on this device".to_string())
        } else {
            match &s.server_url {
                Some(url) => format!("{} · {url}", s.server_type),
                None => s.server_type.clone(),
            }
        };
        Self {
            kind: ItemKind::McpServer,
            tier: if local { TrustTier::Code } else { TrustTier::Content },
            name: s.name,
            detail,
            target: "Connector",
            source_path: source_path.into(),
        }
    }
}

/// A stdio connector's launch spec (`{command, args, env}`) as one command
/// line: each word quoted the way a shell reads it when it has to be, then
/// the names of the variables it sets.
fn command_line(spec: &serde_json::Value) -> String {
    let words = std::iter::once(&spec["command"])
        .chain(spec["args"].as_array().into_iter().flatten())
        .map(|w| w.as_str().map(str::to_string).unwrap_or_else(|| w.to_string()))
        .map(|w| shell_word(&w))
        .collect::<Vec<_>>()
        .join(" ");
    let env: Vec<&str> = spec["env"].as_object().into_iter().flatten().map(|(k, _)| k.as_str()).collect();
    if env.is_empty() { words } else { format!("{words} · sets {}", env.join(", ")) }
}

fn shell_word(w: &str) -> String {
    let plain = !w.is_empty()
        && w.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=@+,%~".contains(c));
    if plain { w.to_string() } else { format!("'{}'", w.replace('\'', "'\\''")) }
}

/// The dry-run result: everything found in an install and what it maps to.
///
/// Building a manifest never writes to Nebo and never modifies the source.
#[derive(Debug, Clone, Serialize)]
pub struct ImportManifest {
    pub source: SourceKind,
    pub root: String,
    pub items: Vec<ImportItem>,
    /// Read-only notes about things found but not (fully) importable — so the
    /// dry-run is honest instead of silently dropping them.
    pub notes: Vec<String>,
}

impl ImportManifest {
    pub fn new(source: SourceKind, root: String) -> Self {
        Self {
            source,
            root,
            items: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn push(&mut self, item: ImportItem) {
        self.items.push(item);
    }

    pub fn note(&mut self, msg: impl Into<String>) {
        self.notes.push(msg.into());
    }

    /// Count of items of a given kind — for the "4 employees · 27 skills" summary.
    pub fn count(&self, kind: ItemKind) -> usize {
        self.items.iter().filter(|i| i.kind == kind).count()
    }

    /// True if any item runs code or pulls a remote payload — i.e. the import
    /// should surface an explicit confirm rather than applying silently.
    pub fn needs_confirmation(&self) -> bool {
        self.items
            .iter()
            .any(|i| !matches!(i.tier, TrustTier::Content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A connector that runs here shows the whole command line it starts,
    /// quoted where a shell would need it, and names what it sets without
    /// its values.
    #[test]
    fn a_local_connector_shows_its_exact_command() {
        let spec = serde_json::json!({
            "command": "/opt/My Tools/server",
            "args": ["--root", "it's here", "-v"],
            "env": { "API_KEY": "sk-never-shown", "REGION": "us" },
        });
        let line = command_line(&spec);
        assert_eq!(line, "'/opt/My Tools/server' --root 'it'\\''s here' -v · sets API_KEY, REGION");
        assert!(!line.contains("sk-never-shown"));
        assert_eq!(command_line(&serde_json::json!({ "command": "uvx", "args": [] , "env": {} })), "uvx");
    }
}
