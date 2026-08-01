//! The dry-run data model: what an import *would* do, before it does anything.
//!
//! A manifest is produced by walking a foreign install read-only. It is the
//! single thing the "point Nebo at an install" UX renders and the user approves.

use serde::Serialize;

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
pub struct ImportItem {
    pub kind: ItemKind,
    pub tier: TrustTier,
    /// Human-facing name: server name, skill name, file stem, or secret key.
    pub name: String,
    /// One-line specifics for the manifest. Never contains a secret *value*.
    pub detail: String,
    /// What this becomes in Nebo, e.g. "MCP integration", "Nebo skill".
    pub target: &'static str,
    /// Path relative to the install root, for provenance in the dry-run.
    pub source_path: String,
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
