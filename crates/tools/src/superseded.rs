//! A retired multi-service plugin steps aside, one service at a time, for
//! the plugins that replace it.
//!
//! The Google Workspace bundle (`gws`) is retired in the marketplace in
//! favour of one plugin per Google service. Bots that installed it keep it,
//! and on a bot that also has the replacement for a service, a request for
//! that service reached the bundle first: live (cloud E2E, 2026-10-11) a
//! Google Docs request ran `gws`, raised its "Google Account" connect card
//! and never reached the connected google-docs plugin.
//!
//! So a bundle's service ("area") is retired on this bot once its
//! replacement is installed, switched on and connected: the bundle's tool
//! loses that area's triggers and skills from its description, its skills
//! for the area leave the skill listing and search, and its commands for
//! the area are refused with the replacement's name, before anything runs
//! and before any connect card. Its other areas work as before, and a bot
//! without a connected replacement sees no change at all.
//!
//! Why a table, not a manifest field: the retirement is written only in the
//! marketplace listing's prose. An installed bundle's manifest carries no
//! retirement and no successor, and the bundle is never republished, so
//! nothing on the bot could say which plugin replaces which of its areas.
//! Another retired bundle is one more row.

use std::collections::HashSet;

/// A retired bundle and, for each area it covered, the plugin that
/// replaces it. An area is the bundle's own word for the service: its
/// commands start with it (`gws docs documents get`) and its skills for it
/// are named after it (`gws-docs`, `gws-docs-write`).
struct Retired {
    bundle: &'static str,
    areas: &'static [(&'static str, &'static str)],
}

const RETIRED: &[Retired] = &[Retired {
    bundle: "gws",
    areas: &[
        ("gmail", "gmail"),
        ("calendar", "google-calendar"),
        ("drive", "google-drive"),
        ("sheets", "google-sheets"),
        ("docs", "google-docs"),
        ("slides", "google-slides"),
    ],
}];

/// One area of a bundle a connected plugin has taken over on this bot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakenOver {
    pub area: &'static str,
    pub successor: &'static str,
}

/// The areas of `slug` that its replacements have taken over here: each
/// replacement installed, not switched off, and connected — for a plugin
/// that keeps an account per employee, some employee has one; otherwise
/// the plugin is ready. Empty for any plugin that is not a retired bundle.
pub fn taken_over(
    plugin_store: &napp::plugin::PluginStore,
    db_store: &db::Store,
    slug: &str,
) -> Vec<TakenOver> {
    let Some(retired) = RETIRED.iter().find(|r| r.bundle == slug) else {
        return Vec::new();
    };
    let installed = crate::plugin_tool::installed_plugin_slugs(plugin_store, db_store);
    let mut with_accounts: Option<HashSet<String>> = None;
    let mut out = Vec::new();
    for &(area, successor) in retired.areas {
        if !installed.iter().any(|s| s == successor) {
            continue;
        }
        let per_employee = plugin_store
            .get_manifest(successor)
            .and_then(|m| m.auth)
            .is_some_and(|a| a.profile_dir_env.is_some());
        let connected = if per_employee {
            with_accounts
                .get_or_insert_with(|| {
                    db_store
                        .list_all_plugin_account_profiles()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|p| p.plugin_slug)
                        .collect()
                })
                .contains(successor)
        } else {
            plugin_store.is_ready(successor)
        };
        if connected {
            out.push(TakenOver { area, successor });
        }
    }
    out
}

/// Whether `trigger`'s words stand together in one of `others`: "google"
/// in "google docs", "google docs" in itself.
pub fn said_within(trigger: &str, others: &[String]) -> bool {
    let words = |t: &str| -> Vec<String> { t.split_whitespace().map(str::to_lowercase).collect() };
    let mine = words(trigger);
    !mine.is_empty()
        && others.iter().any(|o| words(o).windows(mine.len()).any(|w| w == mine.as_slice()))
}

/// The taken-over area a command for `bundle` works in: its first word, a
/// leading bundle name forgiven as the plugin tool forgives it.
pub fn command_area<'a>(bundle: &str, command: &str, taken: &'a [TakenOver]) -> Option<&'a TakenOver> {
    let mut words = command.split_whitespace();
    let mut first = words.next()?;
    if first.eq_ignore_ascii_case(bundle) {
        first = words.next()?;
    }
    taken.iter().find(|t| t.area.eq_ignore_ascii_case(first))
}

/// The taken-over area a skill of `bundle` documents: `<bundle>-<area>` or
/// `<bundle>-<area>-<anything>`.
pub fn skill_area<'a>(bundle: &str, skill: &str, taken: &'a [TakenOver]) -> Option<&'a TakenOver> {
    let rest = skill.strip_prefix(bundle)?.strip_prefix('-')?;
    taken
        .iter()
        .find(|t| rest == t.area || rest.strip_prefix(t.area).is_some_and(|r| r.starts_with('-')))
}

/// What a call to a taken-over area is told: the plugin that does it here.
pub fn refusal(bundle: &str, what: &str, t: &TakenOver) -> String {
    format!(
        "{what} is not used on this bot: {successor} has replaced {bundle} for {area}. Do it with \
         {tool} (its skills say how) — nothing ran, nothing was asked of the owner, and {bundle} is \
         not the tool for {area} here.",
        successor = t.successor,
        area = t.area,
        tool = crate::plugin_tools::plugin_tool_name(t.successor),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taken() -> Vec<TakenOver> {
        vec![TakenOver { area: "docs", successor: "google-docs" }, TakenOver { area: "gmail", successor: "gmail" }]
    }

    #[test]
    fn a_command_is_in_the_area_it_starts_with() {
        let t = taken();
        assert_eq!(command_area("gws", "docs documents get", &t).unwrap().successor, "google-docs");
        assert_eq!(command_area("gws", "gws docs +write --document x", &t).unwrap().successor, "google-docs");
        assert_eq!(command_area("gws", "gmail +triage", &t).unwrap().successor, "gmail");
        assert!(command_area("gws", "drive files list", &t).is_none(), "drive is not taken over");
        assert!(command_area("gws", "documents get", &t).is_none());
        assert!(command_area("gws", "auth login", &t).is_none());
        assert!(command_area("gws", "", &t).is_none());
    }

    #[test]
    fn a_trigger_said_within_a_successors_is_its() {
        let docs = vec!["google-docs".to_string(), "google docs".to_string()];
        assert!(said_within("google docs", &docs));
        assert!(said_within("Google", &docs));
        assert!(!said_within("google meet", &docs));
        assert!(!said_within("docs google", &docs));
        assert!(!said_within("workspace", &docs));
        assert!(!said_within("", &docs));
    }

    #[test]
    fn a_skill_is_in_the_area_its_name_says() {
        let t = taken();
        assert!(skill_area("gws", "gws-docs", &t).is_some());
        assert!(skill_area("gws", "gws-docs-write", &t).is_some());
        assert!(skill_area("gws", "gws-gmail-triage", &t).is_some());
        assert!(skill_area("gws", "gws-docsify", &t).is_none(), "a word boundary, not a prefix");
        assert!(skill_area("gws", "gws-drive", &t).is_none());
        assert!(skill_area("gws", "gws-shared", &t).is_none());
        assert!(skill_area("gws", "google-docs-documents", &t).is_none(), "the successor's own skills");
        assert!(skill_area("gws", "persona-researcher", &t).is_none());
    }
}
