//! Bundled skills and agents shipped with the Nebo binary.
//!
//! All content is embedded via `include_str!()` and loaded directly from
//! memory at startup. Nothing is extracted to disk — this eliminates
//! the `<data_dir>/bundled/` filesystem attack surface.

// ── Bundled Skills ──────────────────────────────────────────────────

/// Embedded skill definitions: `(name, SKILL.md content)`.
///
/// Loaded directly by the skill `Loader` — no filesystem extraction.
pub const BUNDLED_SKILLS: &[(&str, &str)] = &[
    // Knowledge-work core (self-contained, offline) + system self-management.
    // copy-editing was removed — it's marketing-specific (belongs in a Marketer
    // pack, not the universal default). Reference-heavy skills (nebo-design) and
    // binary-backed ones (nebo-office, neboai) install on first run instead.
    ("deep-research", include_str!("deep-research.md")),
    (
        "context-compression",
        include_str!("context-compression.md"),
    ),
    ("evaluation", include_str!("evaluation.md")),
    ("brainstorming", include_str!("brainstorming.md")),
    ("nebo-onboarding", include_str!("nebo-onboarding.md")),
];

// ── Bundled Agents ──────────────────────────────────────────────────

/// Embedded agent definitions: `(name, AGENT.md, agent.json, manifest.json)`.
///
/// Loaded directly by the `AgentLoader` — no filesystem extraction. Only the
/// primary employee is bundled: a fresh Nebo has no coding employee, no
/// specialist of any kind, until the owner hires one from the marketplace or
/// creates one. A bundled employee can never be deleted (it comes back on
/// every reload), so nothing optional belongs here.
pub const BUNDLED_AGENTS: &[(&str, &str, &str, &str)] = &[(
    "assistant",
    include_str!("agents/assistant/AGENT.md"),
    include_str!("agents/assistant/agent.json"),
    include_str!("agents/assistant/manifest.json"),
)];

#[cfg(test)]
mod bundled_agent_tests {
    use super::*;

    /// The bundle is the primary employee alone, it parses, and it declares
    /// no tool requirements: a fresh Nebo carries no coding employee.
    #[test]
    fn only_the_primary_employee_is_bundled_and_it_requires_no_tools() {
        let mut names = Vec::new();
        for (name, _agent_md, agent_json, manifest_json) in BUNDLED_AGENTS {
            let cfg = napp::agent::parse_agent_config(agent_json).unwrap_or_else(|e| panic!("{name}: {e}"));
            let manifest: serde_json::Value = serde_json::from_str(manifest_json).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(manifest["id"], *name, "manifest id matches the bundle key");
            names.push((*name, cfg.requires.tools.clone()));
        }
        assert_eq!(names, vec![("assistant", Vec::<String>::new())], "{names:?}");
    }
}
