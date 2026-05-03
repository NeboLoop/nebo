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
    ("deep-research", include_str!("deep-research.md")),
    ("copywriting", include_str!("copywriting.md")),
    ("social-content", include_str!("social-content.md")),
    ("cold-email", include_str!("cold-email.md")),
    ("copy-editing", include_str!("copy-editing.md")),
    ("context-compression", include_str!("context-compression.md")),
    ("evaluation", include_str!("evaluation.md")),
    ("brainstorming", include_str!("brainstorming.md")),
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
        "ai-coach",
        include_str!("agents/ai-coach/AGENT.md"),
        include_str!("agents/ai-coach/agent.json"),
        include_str!("agents/ai-coach/manifest.json"),
    ),
    (
        "builder",
        include_str!("agents/builder/AGENT.md"),
        include_str!("agents/builder/agent.json"),
        include_str!("agents/builder/manifest.json"),
    ),
    (
        "chief-of-staff",
        include_str!("agents/chief-of-staff/AGENT.md"),
        include_str!("agents/chief-of-staff/agent.json"),
        include_str!("agents/chief-of-staff/manifest.json"),
    ),
    (
        "competitive-intel",
        include_str!("agents/competitive-intel/AGENT.md"),
        include_str!("agents/competitive-intel/agent.json"),
        include_str!("agents/competitive-intel/manifest.json"),
    ),
    (
        "content-strategist",
        include_str!("agents/content-strategist/AGENT.md"),
        include_str!("agents/content-strategist/agent.json"),
        include_str!("agents/content-strategist/manifest.json"),
    ),
    (
        "daily-briefer",
        include_str!("agents/daily-briefer/AGENT.md"),
        include_str!("agents/daily-briefer/agent.json"),
        include_str!("agents/daily-briefer/manifest.json"),
    ),
    (
        "inbox-manager",
        include_str!("agents/inbox-manager/AGENT.md"),
        include_str!("agents/inbox-manager/agent.json"),
        include_str!("agents/inbox-manager/manifest.json"),
    ),
    (
        "marketing-manager",
        include_str!("agents/marketing-manager/AGENT.md"),
        include_str!("agents/marketing-manager/agent.json"),
        include_str!("agents/marketing-manager/manifest.json"),
    ),
    (
        "nuskin",
        include_str!("agents/nuskin/AGENT.md"),
        include_str!("agents/nuskin/agent.json"),
        include_str!("agents/nuskin/manifest.json"),
    ),
    (
        "outreach-coach",
        include_str!("agents/outreach-coach/AGENT.md"),
        include_str!("agents/outreach-coach/agent.json"),
        include_str!("agents/outreach-coach/manifest.json"),
    ),
    (
        "pirate",
        include_str!("agents/pirate/AGENT.md"),
        include_str!("agents/pirate/agent.json"),
        include_str!("agents/pirate/manifest.json"),
    ),
    (
        "private-investigator",
        include_str!("agents/private-investigator/AGENT.md"),
        include_str!("agents/private-investigator/agent.json"),
        include_str!("agents/private-investigator/manifest.json"),
    ),
    (
        "product-builder",
        include_str!("agents/product-builder/AGENT.md"),
        include_str!("agents/product-builder/agent.json"),
        include_str!("agents/product-builder/manifest.json"),
    ),
    (
        "quality-guard",
        include_str!("agents/quality-guard/AGENT.md"),
        include_str!("agents/quality-guard/agent.json"),
        include_str!("agents/quality-guard/manifest.json"),
    ),
    (
        "research-analyst",
        include_str!("agents/research-analyst/AGENT.md"),
        include_str!("agents/research-analyst/agent.json"),
        include_str!("agents/research-analyst/manifest.json"),
    ),
    (
        "search-visibility-monitor",
        include_str!("agents/search-visibility-monitor/AGENT.md"),
        include_str!("agents/search-visibility-monitor/agent.json"),
        include_str!("agents/search-visibility-monitor/manifest.json"),
    ),
    (
        "seo-auditor",
        include_str!("agents/seo-auditor/AGENT.md"),
        include_str!("agents/seo-auditor/agent.json"),
        include_str!("agents/seo-auditor/manifest.json"),
    ),
    (
        "social-media-manager",
        include_str!("agents/social-media-manager/AGENT.md"),
        include_str!("agents/social-media-manager/agent.json"),
        include_str!("agents/social-media-manager/manifest.json"),
    ),
    (
        "warm-market-coach",
        include_str!("agents/warm-market-coach/AGENT.md"),
        include_str!("agents/warm-market-coach/agent.json"),
        include_str!("agents/warm-market-coach/manifest.json"),
    ),
];

