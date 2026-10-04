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
    // App Studio: the one skill for building any app or game (an employee
    // with a page), from a tracker to a designed game. The body is the
    // mechanics (where files go, the two lanes, verify, the SDK contract);
    // the design method ships as on-demand references in
    // `BUNDLED_SKILL_FILES`. Bundled because the SDK contract and the
    // serving rules live nowhere an employee can read at runtime.
    ("app-studio", include_str!("app-studio/SKILL.md")),
    // How an app goes to the marketplace with the owner, in conversation
    // (App Developer mode): screenshots, the listing, the owner's yes.
    ("publish-an-app", include_str!("publish-an-app.md")),
    // How a file reaches the owner on any device: share_file, never the
    // Desktop, AirDrop or a local server a phone away from home can't reach.
    ("file-delivery", include_str!("file-delivery.md")),
];

/// Files a bundled skill carries beside its SKILL.md: `(skill, relative
/// path, content)`. Read with `read_skill_file` and run with `execute`
/// exactly like an installed skill's files, from memory.
pub const BUNDLED_SKILL_FILES: &[(&str, &str, &str)] = &[
    ("app-studio", "references/brief.md", include_str!("app-studio/references/brief.md")),
    ("app-studio", "references/design-recipe.md", include_str!("app-studio/references/design-recipe.md")),
    ("app-studio", "references/boards-and-assets.md", include_str!("app-studio/references/boards-and-assets.md")),
    ("app-studio", "references/wow-catalog.md", include_str!("app-studio/references/wow-catalog.md")),
    ("app-studio", "references/film-scrub.md", include_str!("app-studio/references/film-scrub.md")),
    ("app-studio", "references/kit.md", include_str!("app-studio/references/kit.md")),
    ("app-studio", "references/games.md", include_str!("app-studio/references/games.md")),
    ("app-studio", "references/gate.md", include_str!("app-studio/references/gate.md")),
    ("app-studio", "references/vite-build.md", include_str!("app-studio/references/vite-build.md")),
    ("app-studio", "references/when-it-breaks.md", include_str!("app-studio/references/when-it-breaks.md")),
    ("app-studio", "references/design-depth.md", include_str!("app-studio/references/design-depth.md")),
    ("app-studio", "references/decisions.md", include_str!("app-studio/references/decisions.md")),
    ("app-studio", "references/lane-a-example.md", include_str!("app-studio/references/lane-a-example.md")),
    ("app-studio", "references/sdk-more.md", include_str!("app-studio/references/sdk-more.md")),
    ("app-studio", "references/app-data.md", include_str!("app-studio/references/app-data.md")),
    ("app-studio", "references/motion.md", include_str!("app-studio/references/motion.md")),
    ("app-studio", "scripts/gate.js", include_str!("app-studio/scripts/gate.js")),
    ("app-studio", "LICENSE-THIRD-PARTY.txt", include_str!("app-studio/LICENSE-THIRD-PARTY.txt")),
];

/// The files bundled with skill `name`, by relative path.
pub fn bundled_files(name: &str) -> impl Iterator<Item = (&'static str, &'static str)> + '_ {
    BUNDLED_SKILL_FILES.iter().filter(move |(s, _, _)| *s == name).map(|(_, p, c)| (*p, *c))
}

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
    fn the_app_studio_skill_loads_with_its_triggers_and_the_real_global() {
        let (_, content) = BUNDLED_SKILLS
            .iter()
            .find(|(k, _)| *k == "app-studio")
            .expect("app-studio is registered");
        let skill = super::super::parse_skill_frontmatter(content.as_bytes()).expect("parses");
        assert_eq!(skill.name, "app-studio");
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
            .find(|(k, _)| *k == "app-studio")
            .expect("app-studio is registered");
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
            .find(|(k, _)| *k == "app-studio")
            .expect("app-studio is registered");
        let bundle = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../app/static/sdk/nebo.global.js");
        let bundle = std::fs::read_to_string(&bundle)
            .unwrap_or_else(|e| panic!("the served SDK bundle must be readable at {}: {e}", bundle.display()));
        for name in [
            "nebo", "identity", "storage", "agents", "janus", "decide", "surfaces", "chat", "a2ui",
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
        // The store is shared with the app's employee, and the page hears
        // its writes: the skill says so and the bundle carries it.
        assert!(content.contains("`storage.onChange(cb)"), "the skill names storage.onChange");
        assert!(content.contains("app_data(action:"), "the skill shows the employee's app_data tool");
        assert!(
            bundle.contains("onChange") && bundle.contains("app_data_changed"),
            "the bundle must carry storage.onChange on the app_data_changed event"
        );
        // Typed decisions: the page's `decide` and the employee's tool reach
        // the one app route.
        assert!(content.contains("`decide({state, questions})"), "the skill names decide");
        assert!(content.contains("decide(state:"), "the skill shows the employee's decide tool");
        assert!(bundle.contains("/janus/decide"), "the bundle must post decisions to the app's janus/decide route");
        // The two renamed at the top level, said as such.
        assert!(content.contains("NeboAppSDK.neboFetch"), "nebo.fetch is exported as neboFetch");
        assert!(content.contains("NeboAppSDK.NeboWebSocket"), "nebo.WebSocket is exported as NeboWebSocket");
    }

    /// After a checkpoint a skill comes back cut to its first 5,000 tokens
    /// (20,000 bytes of what its load returned,
    /// `harness::compact::restore::SKILL_TOKENS`). App Studio is the skill of
    /// the longest sessions, so all of it must come back: the body, plus the
    /// load's own lead (the App Developer notice), under that line. Detail
    /// lives in `references/`, read when a pointer says so.
    #[test]
    fn app_studio_comes_back_whole_after_a_checkpoint() {
        let (_, content) = BUNDLED_SKILLS.iter().find(|(k, _)| *k == "app-studio").expect("registered");
        let body = content.splitn(3, "---").nth(2).expect("frontmatter, then the body");
        let lead = 300; // "Loaded skill 'app-studio'. Developer mode and App Developer mode are now on…"
        let restored = 20_000 - 100; // less the cut note
        assert!(
            body.len() + lead < restored,
            "App Studio's body is {} bytes; past {} a checkpoint cuts it. Move detail to references/.",
            body.len(),
            restored - lead
        );
    }

    /// The paths an app page uses resolve at all three addresses Nebo serves
    /// it from (desktop `neboapp://<id>/`, `/apps/<id>/ui/`, and the phone's
    /// `/t/<bot>/apps/<id>/ui/`). A leading `/` works only on the desktop:
    /// apps built that way were blank on the phone (2026-10-01). So the skill
    /// and the tool both load the SDK relatively, the Vite recipe sets
    /// `base: './'`, and a rename never deletes.
    #[test]
    fn app_studio_teaches_paths_that_work_on_the_phone() {
        let (_, content) = BUNDLED_SKILLS.iter().find(|(k, _)| *k == "app-studio").expect("registered");
        assert!(content.contains(r#"<script src="../../../sdk/nebo.global.js"></script>"#), "the relative SDK tag");
        // The Vite recipe lives in its reference (the skill's top stays inside
        // what a checkpoint restores); the skill points to it.
        let (_, _, vite) = BUNDLED_SKILL_FILES
            .iter()
            .find(|(k, path, _)| *k == "app-studio" && *path == "references/vite-build.md")
            .expect("the Vite recipe ships");
        assert!(content.contains("references/vite-build.md"), "the skill points to the Vite recipe");
        assert!(vite.contains("base: './'"), "Vite builds with relative paths");
        assert!(vite.contains("outDir: '../ui'"), "Vite builds into ui/");
        assert!(content.contains("new_name:"), "rename keeps the employee");
        assert!(
            !content.contains(r#"<script src="/sdk/nebo.global.js">"#),
            "no page in the skill loads the SDK by an absolute path"
        );
        for (_, _, file) in BUNDLED_SKILL_FILES {
            assert!(!file.contains(r#"src="/sdk/nebo.global.js""#), "no reference loads the SDK absolutely");
            assert!(!file.contains("build-an-app") && !file.contains("nebo-app.md"), "no dangling skill names");
        }
        assert!(
            crate::agent_tool::PersonaTool::APP_SDK_SCRIPT.contains(r#"src="../../../sdk/nebo.global.js""#),
            "what create/update_employee tell the employee matches the skill"
        );
    }

    /// Every reference the skill points to ships with it, and its gate takes
    /// the `execute` tool's arguments.
    #[test]
    fn app_studio_ships_every_file_it_names() {
        let (_, content) = BUNDLED_SKILLS.iter().find(|(k, _)| *k == "app-studio").expect("registered");
        let files: Vec<&str> = bundled_files("app-studio").map(|(p, _)| p).collect();
        // The skill points to its references; the design method (one of them)
        // is the map of the studio's own files.
        let method = bundled_files("app-studio").find(|(p, _)| *p == "references/design-depth.md").expect("the method ships").1;
        for named in ["design-depth.md", "decisions.md", "games.md", "lane-a-example.md", "sdk-more.md", "app-data.md", "vite-build.md", "when-it-breaks.md"] {
            assert!(content.contains(&format!("references/{named}")), "the skill points to {named}");
            assert!(files.contains(&format!("references/{named}").as_str()), "references/{named} ships");
        }
        for named in ["brief.md", "design-recipe.md", "boards-and-assets.md", "wow-catalog.md", "film-scrub.md", "kit.md", "gate.md"] {
            assert!(content.contains(named) || method.contains(named), "the skill or its design method points to {named}");
            assert!(files.contains(&format!("references/{named}").as_str()), "references/{named} ships");
        }
        assert!(files.contains(&"scripts/gate.js") && files.contains(&"LICENSE-THIRD-PARTY.txt"));
        let gate = bundled_files("app-studio").find(|(p, _)| *p == "scripts/gate.js").unwrap().1;
        assert!(gate.contains("SKILL_ARGS"), "the gate reads execute's arguments");
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
