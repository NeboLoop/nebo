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
/// Loaded directly by the `AgentLoader` — no filesystem extraction.
pub const BUNDLED_AGENTS: &[(&str, &str, &str, &str)] = &[
    (
        "assistant",
        include_str!("agents/assistant/AGENT.md"),
        include_str!("agents/assistant/agent.json"),
        include_str!("agents/assistant/manifest.json"),
    ),
    (
        "developer",
        include_str!("agents/developer/AGENT.md"),
        include_str!("agents/developer/agent.json"),
        include_str!("agents/developer/manifest.json"),
    ),
];

#[cfg(test)]
mod bundled_agent_tests {
    use super::*;

    /// Every bundled employee parses, and the Developer declares the coding
    /// tool as part of its job while the general employee does not.
    #[test]
    fn bundled_employees_parse_and_the_developer_declares_the_code_tool() {
        let mut names = Vec::new();
        for (name, agent_md, agent_json, manifest_json) in BUNDLED_AGENTS {
            let cfg = napp::agent::parse_agent_config(agent_json).unwrap_or_else(|e| panic!("{name}: {e}"));
            let manifest: serde_json::Value = serde_json::from_str(manifest_json).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(manifest["id"], *name, "manifest id matches the bundle key");
            // The Developer is new copy under the no-em-dash rule; the Assistant's
            // persona is approved copy and stays verbatim.
            if *name == "developer" {
                assert!(!agent_md.contains('\u{2014}'), "{name}: no em-dash in an owner-visible AGENT.md");
            }
            names.push((*name, cfg.requires.tools.clone()));
        }
        assert!(names.iter().any(|(n, t)| *n == "developer" && t == &["code".to_string()]), "{names:?}");
        assert!(names.iter().any(|(n, t)| *n == "assistant" && t.is_empty()), "{names:?}");
    }
}
