//! The one-time move out of the old single agent profile. Its Identity,
//! Personality and Rules pages are gone, and what they held reached every
//! employee's prompt where the owner could no longer see or edit it. Each
//! field moves to the place the owner edits it today, and the prompt stops
//! reading the profile:
//!
//! | Old field | Goes to |
//! |---|---|
//! | personality (custom text or preset), creature, role, vibe, emoji, voice, formality, response length, emoji usage | each employee's soul (Employee settings) |
//! | rules | the company layer: an `always` rule every employee reads |
//! | tool notes | the company layer: an `always` rule every employee reads |
//!
//! It runs at startup after the stored tool names are converted (those read
//! the profile's text too) and before any employee is loaded, so the souls
//! the employees start with are the moved ones. The company rules are
//! written through [`napp::commit_change`] like every pack change; the
//! caller applies that one pack, since the owner already decided these rules
//! apply to every employee.

use std::path::Path;

use db::models::AgentProfile;
use serde_json::json;
use tracing::info;
use types::NeboError;

/// The name the move is recorded under.
pub const CONVERSION: &str = "old_profile_moved_v1";

/// The profile's style values that said nothing: the old prompt printed them
/// ("Voice: neutral"), but a soul only carries a choice.
const UNCHOSEN: &[(&str, &str)] = &[
    ("Voice", "neutral"),
    ("Formality", "adaptive"),
    ("Response length", "adaptive"),
    ("Emoji usage", "moderate"),
];

/// Run the move once. Returns the company pack's slug when rules or tool
/// notes were written to it, for the caller to apply.
pub fn upgrade(store: &db::Store, packs_dir: &Path) -> Result<Option<String>, NeboError> {
    if store.upgrade_conversion_done(CONVERSION)? {
        return Ok(None);
    }
    let profile = store.get_agent_profile()?;
    let persona = profile.as_ref().and_then(persona);
    let rules: Vec<(&str, &str, String)> = profile
        .as_ref()
        .map(|p| {
            [
                ("standing-instructions", "Standing instructions", p.agent_rules.as_deref()),
                ("tool-notes", "Tool notes", p.tool_notes.as_deref()),
            ]
            .into_iter()
            .filter_map(|(file, title, text)| {
                let text = structured_or_raw(text?.trim());
                (!text.is_empty()).then_some((file, title, text))
            })
            .collect()
        })
        .unwrap_or_default();

    let pack = if rules.is_empty() {
        None
    } else {
        Some(write_company_rules(packs_dir, &rules).map_err(|e| NeboError::Internal(format!("company layer: {e}")))?)
    };

    let souls: Vec<(String, String)> = match &persona {
        None => Vec::new(),
        Some(persona) => store
            .list_agents(10_000, 0)?
            .into_iter()
            .filter(|a| a.kind.as_deref() != Some("linked") && a.is_app.unwrap_or(0) == 0)
            .map(|a| {
                let moved = persona.replace("{agent_name}", &a.name);
                let soul = match a.soul.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                    Some(existing) => format!("{existing}\n\n{moved}"),
                    None => moved,
                };
                (a.id, soul)
            })
            .collect(),
    };

    let report = json!({
        "persona": persona,
        "souls": souls.iter().map(|(id, _)| id).collect::<Vec<_>>(),
        "company_pack": pack,
        "rules": rules.iter().map(|(_, title, text)| json!({ "title": title, "text": text })).collect::<Vec<_>>(),
    });
    store.retire_agent_profile(&souls, CONVERSION, &report.to_string())?;
    info!(
        souls = souls.len(),
        company_pack = pack.as_deref().unwrap_or(""),
        rules = rules.len(),
        "old agent profile moved: persona to the employees' souls, rules and tool notes to the company layer"
    );
    Ok(pack)
}

/// The profile's persona as soul text, `{agent_name}` left for each
/// employee's own name. `None` when the owner chose nothing.
fn persona(p: &AgentProfile) -> Option<String> {
    let text = |v: &Option<String>| v.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let personality = text(&p.custom_personality).or_else(|| preset(p.personality_preset.as_deref()).map(str::to_string));
    let mut lines: Vec<String> = [("Creature", &p.creature), ("Role", &p.role), ("Vibe", &p.vibe), ("Emoji", &p.emoji)]
        .into_iter()
        .filter_map(|(label, v)| text(v).map(|v| format!("{label}: {v}")))
        .collect();
    for ((label, unchosen), v) in UNCHOSEN.iter().zip([&p.voice_style, &p.formality, &p.response_length, &p.emoji_usage]) {
        if let Some(v) = text(v).filter(|v| v != unchosen) {
            lines.push(format!("{label}: {v}"));
        }
    }
    let parts: Vec<String> = personality.into_iter().chain((!lines.is_empty()).then(|| lines.join("\n"))).collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// The text the old presets put in the prompt. `balanced` had none.
fn preset(name: Option<&str>) -> Option<&'static str> {
    match name? {
        "professional" => Some("You are professional, precise, and efficient. You focus on accuracy and clear communication."),
        "friendly" => Some("You are warm, friendly, and approachable. You make people feel comfortable and supported."),
        "casual" => Some("You are laid-back and casual. You keep things light and conversational."),
        "creative" => Some("You are creative, imaginative, and expressive. You bring fresh perspectives and ideas."),
        "analytical" => Some(
            "You are methodical, detail-oriented, and data-driven. You think critically and provide thorough analysis.",
        ),
        _ => None,
    }
}

/// Rules and notes were saved either as text or as a JSON list (of strings,
/// or of objects with `text`, `rule` or `note`); a list becomes one bullet
/// per item, as the old prompt showed it.
fn structured_or_raw(text: &str) -> String {
    if let Ok(items) = serde_json::from_str::<Vec<String>>(text) {
        return items.iter().map(|item| format!("- {item}")).collect::<Vec<_>>().join("\n");
    }
    if let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(text) {
        let lines: Vec<String> = items
            .iter()
            .filter_map(|item| {
                item.get("text")
                    .or_else(|| item.get("rule"))
                    .or_else(|| item.get("note"))
                    .and_then(|v| v.as_str())
                    .map(|s| format!("- {s}"))
            })
            .collect();
        if !lines.is_empty() {
            return lines.join("\n");
        }
    }
    text.to_string()
}

/// Write each `(file, title, text)` as an `always` rule in the company layer:
/// into the company pack when there is one, else into a new one. A file
/// already there with the same text is kept (a retried move writes nothing
/// twice); a different file of that name is left alone and the rule takes
/// the next free name. Returns the pack's slug.
fn write_company_rules(packs_dir: &Path, rules: &[(&str, &str, String)]) -> Result<String, napp::PackError> {
    std::fs::create_dir_all(packs_dir)?;
    let existing = napp::scan_packs(packs_dir).into_iter().find(|p| p.layer == napp::PackLayer::Company);
    let slug = match &existing {
        Some(pack) => pack.slug.clone(),
        None => (1..)
            .map(|n| if n == 1 { "company".to_string() } else { format!("company-{n}") })
            .find(|slug| !packs_dir.join(slug).exists())
            .expect("a free slug"),
    };
    napp::commit_change(&packs_dir.join(&slug), |staging| {
        match &existing {
            Some(pack) => napp::copy_tree(&pack.source_path, staging)?,
            None => std::fs::write(
                staging.join("COMPANY.md"),
                format!(
                    "---\ntype: \"pack\"\nscope: \"company\"\ncompany: \"{slug}\"\nname: \"Company\"\nversion: \"0.1.0\"\ncapabilities: []\n---\n\n\
                     # Company\n\nHow this company works: the standing instructions every employee works by.\n"
                ),
            )?,
        }
        let dir = staging.join("rules");
        std::fs::create_dir_all(&dir)?;
        for (file, title, text) in rules {
            let content = format!("---\nrule: {}\nalways: true\n---\n\n{text}\n", json!(title));
            let path = (1..)
                .map(|n| dir.join(if n == 1 { format!("{file}.md") } else { format!("{file}-{n}.md") }))
                .find(|path| std::fs::read_to_string(path).map_or(true, |on_disk| on_disk == content))
                .expect("a free name");
            std::fs::write(path, &content)?;
        }
        Ok(())
    })?;
    Ok(slug)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &Path) -> db::Store {
        db::Store::new(&dir.join("t.db").to_string_lossy()).unwrap()
    }

    fn employee(store: &db::Store, id: &str, name: &str, kind: Option<&str>, soul: Option<&str>) {
        store.create_agent(id, kind, name, "", "", "", None, None).unwrap();
        store
            .update_agent(id, name, "", "", "", None, None, soul, None, None, None, None, None, None, None)
            .unwrap();
    }

    fn profile(store: &db::Store) {
        store.ensure_agent_profile().unwrap();
        store
            .update_agent_profile(
                None,
                Some("friendly"),
                Some("You are {agent_name}, calm under pressure."),
                Some("neutral"),
                Some("brief"),
                None,
                None,
                None,
                None,
                Some("owl"),
                None,
                None,
                None,
                Some(r#"["Never book travel without asking.","Sign emails with the owner's name."]"#),
                Some("Use the shared drive for client files."),
                None,
                None,
            )
            .unwrap();
    }

    /// A profile with a persona, rules and tool notes: the persona lands in
    /// every employee's soul (a linked employee's soul is its runtime's and
    /// is left alone), the rules and notes become `always` rules in a new
    /// company pack that loads, the profile keeps none of it, and the old
    /// prompt path no longer shows any of it.
    #[test]
    fn a_filled_profile_moves_to_the_souls_and_the_company_layer() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let packs = dir.path().join("packs");
        profile(&store);
        employee(&store, "assistant", "Ava", None, None);
        employee(&store, "books", "Bea", None, Some("Dry humour."));
        employee(&store, "linked", "Lee", Some("linked"), Some("Theirs."));

        let slug = upgrade(&store, &packs).unwrap().expect("rules were written");

        let soul = |id: &str| store.get_agent(id).unwrap().unwrap().soul.unwrap_or_default();
        assert_eq!(soul("assistant"), "You are Ava, calm under pressure.\n\nCreature: owl\nResponse length: brief");
        assert_eq!(soul("books"), "Dry humour.\n\nYou are Bea, calm under pressure.\n\nCreature: owl\nResponse length: brief");
        assert_eq!(soul("linked"), "Theirs.");

        let pack = napp::load_pack(&packs.join(&slug)).unwrap();
        assert_eq!(pack.layer, napp::PackLayer::Company);
        let rules: Vec<(&str, &str, bool)> =
            pack.rules.iter().map(|r| (r.entry.title.as_str(), r.entry.body.trim(), r.always)).collect();
        assert_eq!(
            rules,
            vec![
                ("Standing instructions", "- Never book travel without asking.\n- Sign emails with the owner's name.", true),
                ("Tool notes", "Use the shared drive for client files.", true),
            ]
        );

        let p = store.get_agent_profile().unwrap().unwrap();
        for moved in [&p.custom_personality, &p.creature, &p.agent_rules, &p.tool_notes] {
            assert_eq!(moved.as_deref(), Some(""));
        }
        assert_eq!(p.personality_preset.as_deref(), Some("balanced"));
        assert_eq!(p.response_length.as_deref(), Some("adaptive"));

        let ctx = agent::db_context::load_db_context(&store, "", "assistant", &[]);
        let prompt = agent::db_context::format_for_system_prompt(&ctx, "Ava");
        for old in ["calm under pressure", "warm, friendly", "owl", "brief", "Never book travel", "shared drive"] {
            assert!(!prompt.contains(old), "{old:?} still reaches the prompt through the old profile");
        }

        assert!(store.upgrade_conversion_done(CONVERSION).unwrap());
        assert_eq!(upgrade(&store, &packs).unwrap(), None, "the move runs once");
    }

    /// With a company pack already there, the rules join it and everything
    /// it held stays; a retried write of the same rule is not a second file.
    #[test]
    fn the_rules_join_the_company_pack_that_is_there() {
        let dir = tempfile::tempdir().unwrap();
        let packs = dir.path().join("packs");
        let ours = packs.join("bright-carpet");
        std::fs::create_dir_all(ours.join("rules")).unwrap();
        std::fs::write(ours.join("COMPANY.md"), "---\nname: Bright Carpet\n---\n\nWe clean carpets.\n").unwrap();
        std::fs::write(ours.join("rules/deposits.md"), "---\nrule: Deposits\nalways: true\n---\n\nDeposit first.\n").unwrap();
        std::fs::write(ours.join("rules/tool-notes.md"), "---\nrule: Something else\n---\n\nOwner's own.\n").unwrap();
        let rules = vec![("tool-notes", "Tool notes", "Use the shared drive.".to_string())];

        assert_eq!(write_company_rules(&packs, &rules).unwrap(), "bright-carpet");
        assert_eq!(write_company_rules(&packs, &rules).unwrap(), "bright-carpet");

        let pack = napp::load_pack(&ours).unwrap();
        let titles: Vec<&str> = pack.rules.iter().map(|r| r.entry.title.as_str()).collect();
        // Sorted by file: `tool-notes-2.md` comes before `tool-notes.md`.
        assert_eq!(titles, vec!["Deposits", "Tool notes", "Something else"]);
        assert_eq!(pack.body.trim(), "We clean carpets.");
        assert!(ours.join("rules/tool-notes-2.md").is_file());
        assert!(!ours.join("rules/tool-notes-3.md").exists(), "the retry wrote nothing twice");
    }

    #[test]
    fn rules_saved_as_a_list_become_bullets() {
        assert_eq!(structured_or_raw(r#"["One","Two"]"#), "- One\n- Two");
        assert_eq!(structured_or_raw(r#"[{"text":"Do this"},{"rule":"Do that"},{"note":"Mind this"}]"#), "- Do this\n- Do that\n- Mind this");
        assert_eq!(structured_or_raw("Just plain text rules"), "Just plain text rules");
    }

    /// An untouched profile (the default `balanced` preset and style values)
    /// moves nothing: no soul changes, no company pack, and it is recorded.
    #[test]
    fn an_untouched_profile_moves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let packs = dir.path().join("packs");
        store.ensure_agent_profile().unwrap();
        employee(&store, "assistant", "Ava", None, Some("Mine."));

        assert_eq!(upgrade(&store, &packs).unwrap(), None);
        assert_eq!(store.get_agent("assistant").unwrap().unwrap().soul.as_deref(), Some("Mine."));
        assert!(!packs.join("company").exists());
        assert!(store.upgrade_conversion_done(CONVERSION).unwrap());
    }
}
