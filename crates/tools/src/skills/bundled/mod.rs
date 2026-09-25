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
    ("staff-a-business", include_str!("staff-a-business.md")),
    // How a company's industry, franchise, and company layers get written.
    // Bundled because a fresh Nebo must know the procedure before it has a
    // company: without it the owner would hand-write the markdown folders.
    ("company-layers", include_str!("company-layers.md")),
    // How an app (an employee with a page) is created, wired to the SDK
    // global, and iterated on. Bundled because the SDK contract lives
    // nowhere an employee can read at runtime; without it every app page
    // is written against a global that does not exist.
    ("build-an-app", include_str!("build-an-app.md")),
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
mod bundled_skill_tests {
    use super::*;

    /// Every bundled skill parses the way the loader parses it, and its
    /// frontmatter name is the key it is registered under: the loader keys
    /// both its catalog and its lazy template index by the frontmatter name,
    /// so a mismatch loads a skill nobody can name.
    #[test]
    fn every_bundled_skill_parses_and_is_keyed_by_its_own_name() {
        let mut names = Vec::new();
        for (key, content) in BUNDLED_SKILLS {
            let skill = super::super::parse_skill_frontmatter(content.as_bytes())
                .unwrap_or_else(|e| panic!("{key}: {e}"));
            assert_eq!(&skill.name, key, "bundled key is the skill's own name");
            assert!(!skill.description.trim().is_empty(), "{key} has no description");
            names.push(skill.name);
        }
        assert!(
            names.contains(&"company-layers".to_string()),
            "the company-layers procedure ships with the binary: {names:?}"
        );
    }

    /// The company layers skill names the six standards the runtime itself
    /// reads. They are spelled the same in every company or Nebo cannot read
    /// a stranger's company at all, so a typo here is a silently unbounded
    /// workforce, not a documentation error. Kept in step with
    /// `nebo-server`'s `layers_update::company_ids`.
    #[test]
    fn the_company_layers_skill_spells_the_runtime_ids_exactly() {
        let (_, skill) = BUNDLED_SKILLS
            .iter()
            .find(|(k, _)| *k == "company-layers")
            .expect("company-layers is registered");
        for id in [
            "company.unattended.spend_per_day_cents",
            "company.unattended.spend_per_counterparty_day_cents",
            "company.unattended.spend_per_operation_cents",
            "company.unattended.irreversible_per_day",
            "company.unattended.grant_freshness_secs",
            "company.owner.pages",
        ] {
            assert!(skill.contains(id), "the skill must name `{id}`");
        }
        // A pack is knowledge: the loader refuses one that carries a skill,
        // and the procedure has to say so before an employee tries it.
        assert!(skill.contains("SKILL.md"), "the skill must say a pack never holds a SKILL.md");
    }

    /// The app skill loads, fires on the words an owner actually says, and
    /// names the real SDK global. The served bundle is an IIFE assigned to
    /// `NeboAppSDK`; a page written against a bare `nebo` global throws, so
    /// the skill has to spell the global and warn off the wrong one.
    #[test]
    fn the_build_an_app_skill_loads_with_its_triggers_and_the_real_global() {
        let (_, content) = BUNDLED_SKILLS
            .iter()
            .find(|(k, _)| *k == "build-an-app")
            .expect("build-an-app is registered");
        let skill = super::super::parse_skill_frontmatter(content.as_bytes()).expect("parses");
        assert_eq!(skill.name, "build-an-app");
        for trigger in ["make an app", "dashboard", "app interface"] {
            assert!(
                skill.triggers.iter().any(|t| t == trigger),
                "trigger `{trigger}` missing from {:?}",
                skill.triggers
            );
        }
        assert!(content.contains("NeboAppSDK.nebo.identity.get()"), "the skill shows the real global");
        assert!(content.contains("/sdk/nebo.global.js"), "the skill loads the served bundle");
        assert!(
            content.contains("delete_employee(name:"),
            "the skill sends deletion through the registry door, not the folder"
        );
    }

    /// ONE door to change an app, and it is the tool. The skill used to send
    /// the employee to the file tool to rewrite ui/ by hand while the tool
    /// description said never to hand-write the files (2026-09-19); an
    /// employee reading both had two contradictory procedures.
    #[test]
    fn the_app_skill_changes_an_app_through_the_tool_not_the_file_door() {
        let (_, content) = BUNDLED_SKILLS
            .iter()
            .find(|(k, _)| *k == "build-an-app")
            .expect("build-an-app is registered");
        let iterate = content
            .split("## Iterate")
            .nth(1)
            .expect("the skill has an Iterate section");
        assert!(
            iterate.contains("update_employee("),
            "Iterate must send the change through update_employee: {iterate}"
        );
        assert!(
            !iterate.contains("file tool"),
            "Iterate must not send the employee to the file tool: {iterate}"
        );
        // The name is the folder, never the id the page is served under.
        assert!(
            content.contains("It is NOT\n  the app id"),
            "the skill must say the name is not the app id"
        );
    }

    /// Every SDK name the skill promises is in the bundle the page loads.
    /// The contract lives in the skill alone — when it drifts from
    /// `app/static/sdk/nebo.global.js`, a page written from it throws.
    #[test]
    fn every_sdk_name_the_skill_promises_is_exported_by_the_served_bundle() {
        let (_, content) = BUNDLED_SKILLS
            .iter()
            .find(|(k, _)| *k == "build-an-app")
            .expect("build-an-app is registered");
        let bundle = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../app/static/sdk/nebo.global.js");
        let bundle = std::fs::read_to_string(&bundle)
            .unwrap_or_else(|e| panic!("the served SDK bundle must be readable at {}: {e}", bundle.display()));
        for name in [
            "nebo", "identity", "storage", "agents", "janus", "surfaces", "chat", "a2ui",
            "neboFetch", "NeboWebSocket", "NeboSDK", "NeboSurfaces", "NeboA2UI", "getAppId",
            "getBaseUrl", "setAppId", "setBaseUrl",
        ] {
            assert!(
                content.contains(&format!("`{name}`")) || content.contains(&format!("NeboAppSDK.{name}")),
                "the skill must name the export `{name}`"
            );
            assert!(
                bundle.contains(&format!(".{name}=")),
                "the bundle does not export `{name}` — the skill's contract has drifted"
            );
        }
        // The two renamed at the top level, said as such.
        assert!(content.contains("NeboAppSDK.neboFetch"), "nebo.fetch is exported as neboFetch");
        assert!(content.contains("NeboAppSDK.NeboWebSocket"), "nebo.WebSocket is exported as NeboWebSocket");
    }
}

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
