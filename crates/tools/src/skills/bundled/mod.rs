//! Bundled skills and agents shipped with the Nebo binary.
//!
//! All content is embedded via `include_str!()` and loaded directly from
//! memory at startup. Nothing is extracted to disk — this eliminates
//! the `~/.nebo/bundled/` filesystem attack surface.

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
pub const BUNDLED_AGENTS: &[(&str, &str, &str, &str)] = &[(
    "assistant",
    include_str!("agents/assistant/AGENT.md"),
    include_str!("agents/assistant/agent.json"),
    include_str!("agents/assistant/manifest.json"),
)];
