//! A provider the owner adds runs only the models he typed on it (owner,
//! 2026-10-11). Before, Nebo filled in a model for him: a connection with
//! none ran the catalog's default for its vendor, and every boot seeded the
//! vendor catalogs into `provider_models` under the bare vendor name
//! (`anthropic`), where a pack or a chat could name them.
//!
//! [`upgrade`] makes what each owner runs today his own, once, so nothing
//! changes for him: a connection with no model gets the one it ran on, and
//! every model a pack, a chat, a session, an employee or the bot's default
//! names on one of his connections becomes a model he added there. The
//! seeded vendor rows nobody used are then removed, and the vendor catalogs
//! leave his `models.yaml`. It is idempotent: a second run finds nothing to do.

use std::collections::BTreeMap;

use tracing::{info, warn};

/// The vendors whose models Nebo seeded from its catalog.
const SEEDED_VENDORS: &[&str] = &["anthropic", "openai", "google", "deepseek"];

/// The model each vendor's connection ran when it named none and the
/// owner's `models.yaml` said nothing about it: what the embedded catalog's
/// default rule gave before it was removed (Anthropic from its old
/// fallback list, every other vendor its first catalog row). Read only to
/// write that model down as the owner's.
const RAN_BEFORE: &[(&str, &str)] = &[
    ("anthropic", "claude-sonnet-4-6"),
    ("openai", "gpt-5.4"),
    ("google", "gemini-3.1-pro-preview"),
    ("deepseek", "deepseek-chat"),
];

/// What [`upgrade`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Connections that named no model and now name the one they ran.
    pub pinned: usize,
    /// Models in use that became models the owner added on a connection.
    pub kept: usize,
    /// Seeded vendor rows removed (nobody used them, or now kept as added).
    pub removed: usize,
}

/// Run the upgrade on this bot's store and the owner's models.yaml.
pub fn upgrade(store: &db::Store) {
    let path = config::data_dir().ok().map(|d| d.join(types::constants::files::MODELS_YAML));
    let owner: Option<serde_yaml::Value> =
        path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_yaml::from_str(&t).ok());
    let counts = own_models(store, owner.as_ref());
    if counts != Counts::default() {
        info!(pinned = counts.pinned, kept = counts.kept, removed = counts.removed, "provider models are the owner's own");
    }
    if let Some(mut cfg) = config::ModelsConfig::load_owned() {
        let before = cfg.providers.len();
        cfg.providers.retain(|vendor, _| !SEEDED_VENDORS.contains(&vendor.as_str()));
        if cfg.providers.len() != before
            && let Err(e) = cfg.save()
        {
            warn!(error = %e, "models.yaml still lists vendor models; tried again at the next start");
        }
    }
}

/// The model a vendor's connection ran when it named none: the owner's
/// `defaults.primary` when it named the vendor, then his first fallback
/// that did, then his catalog's first row for it, then [`RAN_BEFORE`].
fn ran_before(owner: Option<&serde_yaml::Value>, vendor: &str) -> Option<String> {
    let of = |spec: &str| spec.split_once('/').filter(|(p, _)| *p == vendor).map(|(_, m)| m.to_string());
    if let Some(owner) = owner {
        if let Some(m) = owner["defaults"]["primary"].as_str().and_then(of) {
            return Some(m);
        }
        if let Some(m) = owner["defaults"]["fallbacks"].as_sequence().into_iter().flatten().filter_map(|v| v.as_str()).find_map(of) {
            return Some(m);
        }
        if let Some(m) = owner["providers"][vendor][0]["id"].as_str() {
            return Some(m.to_string());
        }
    }
    RAN_BEFORE.iter().find(|(v, _)| *v == vendor).map(|(_, m)| m.to_string())
}

/// The work of [`upgrade`] on the store, with the owner's file as read.
pub fn own_models(store: &db::Store, owner: Option<&serde_yaml::Value>) -> Counts {
    let mut counts = Counts::default();
    let mut profiles = store.list_auth_profiles().unwrap_or_default();

    // A connection that named no model gets the one it ran on.
    for p in profiles.iter_mut().filter(|p| SEEDED_VENDORS.contains(&p.provider.as_str())) {
        if p.model.as_deref().is_some_and(|m| !m.trim().is_empty()) {
            continue;
        }
        if let Some(model) = ran_before(owner, &p.provider)
            && store.set_auth_profile_model(&p.id, &model).is_ok()
        {
            p.model = Some(model);
            counts.pinned += 1;
        }
    }

    // The connection a bare vendor name (`anthropic/…`) meant: the first
    // active one of that vendor, as the providers are loaded.
    let mut bare: BTreeMap<String, String> = BTreeMap::new();
    for p in profiles.iter().filter(|p| p.is_active.unwrap_or(0) == 1) {
        bare.entry(p.provider.clone()).or_insert_with(|| crate::handlers::connections::connection_key(p));
    }

    // Every model the owner's choices name.
    let mut named: Vec<String> = store.stored_model_choices().unwrap_or_default();
    for pack in store.list_intelligence_packs().unwrap_or_default() {
        if let Ok(serde_json::Value::Object(levels)) = serde_json::to_value(&pack.levels) {
            named.extend(levels.values().filter_map(|v| v.as_str().map(str::to_string)));
        }
    }
    if let Some(owner) = owner {
        for spec in [&owner["defaults"]["primary"], &owner["task_routing"]["general"]] {
            named.extend(spec.as_str().map(str::to_string));
        }
    }
    for p in &profiles {
        if let Some(m) = p.model.as_deref().filter(|m| !m.trim().is_empty()) {
            named.push(format!("{}/{m}", crate::handlers::connections::connection_key(p)));
        }
    }

    // Each becomes a model the owner added on its connection, with what the
    // seeded row knew about it.
    for spec in named {
        let Some((who, model)) = spec.split_once('/') else { continue };
        let vendor = who.split('@').next().unwrap_or(who);
        if !SEEDED_VENDORS.contains(&vendor) {
            continue;
        }
        let key = if who.contains('@') {
            if !profiles.iter().any(|p| crate::handlers::connections::connection_key(p) == who) {
                continue;
            }
            who.to_string()
        } else {
            match bare.get(vendor) {
                Some(key) => key.clone(),
                None => continue,
            }
        };
        let own = store.list_provider_models(&key).unwrap_or_default();
        if own.iter().any(|m| m.model_id == model) {
            continue;
        }
        let seeded = store.list_provider_models(vendor).unwrap_or_default().into_iter().find(|m| m.model_id == model);
        let (name, window, caps) = match &seeded {
            Some(m) => (m.display_name.clone(), m.context_window, m.capabilities.clone().unwrap_or_else(|| "[]".into())),
            None => (model.to_string(), None, "[]".to_string()),
        };
        if store.put_connection_model(&key, model, &name, "chat", window, &caps, "added", true).is_ok() {
            counts.kept += 1;
        }
    }

    // The seeded vendor rows go.
    for vendor in SEEDED_VENDORS {
        let rows = store.list_provider_models(vendor).map(|r| r.len()).unwrap_or(0);
        if rows > 0 && store.delete_provider_models_by_provider(vendor).is_ok() {
            counts.removed += rows;
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, db::Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = db::Store::new(&dir.path().join("nebo.db").to_string_lossy()).unwrap();
        (dir, store)
    }

    fn seed(store: &db::Store, vendor: &str, model: &str) {
        store
            .upsert_provider_model(
                &format!("{vendor}/{model}"),
                vendor,
                model,
                &format!("{model} name"),
                None,
                Some(200_000),
                None,
                None,
                Some("[\"vision\",\"thinking\"]"),
                None,
                Some("t"),
                true,
            )
            .unwrap();
    }

    /// What each owner runs today stays his: a connection with no model gets
    /// the one it ran; a model a pack or a chat names becomes one he added,
    /// with what the seeded row knew; the seeded rows nobody named go; a
    /// second run does nothing.
    #[test]
    fn what_each_owner_runs_becomes_his_own_and_the_seed_goes() {
        let (_dir, store) = store();
        store.create_auth_profile("a1", "Work", "anthropic", "k", None, None, 10, 1, None, None).unwrap();
        store.create_auth_profile("o1", "Mine", "openai", "k", Some("model-o"), None, 10, 1, None, None).unwrap();
        seed(&store, "anthropic", "model-a");
        seed(&store, "anthropic", "model-b");
        seed(&store, "anthropic", "model-unused");
        store.put_connection_model("openai@o1", "model-o", "model-o", "chat", None, "[]", "added", true).unwrap();
        let pack = types::packs::Pack {
            id: "p".into(),
            name: "Mine".into(),
            levels: types::packs::PackLevels { medium: Some("anthropic@a1/model-b".into()), ..Default::default() },
            fallback: true,
            built_in: false,
            lanes: Default::default(),
            level_effort: Default::default(),
        };
        store.save_intelligence_pack(&pack).unwrap();
        store.create_chat_for_session("c", "agent:x:thread:c", "C", None).unwrap();
        store.set_chat_model("c", Some("anthropic/model-a")).unwrap();
        let owner: serde_yaml::Value = serde_yaml::from_str("defaults:\n  primary: pack/nebo-ai\n  fallbacks: [anthropic/model-a]\n").unwrap();

        let counts = own_models(&store, Some(&owner));

        assert_eq!(store.get_auth_profile("a1").unwrap().unwrap().model.as_deref(), Some("model-a"), "the model it ran");
        let own: Vec<(String, Option<i64>, String)> = store
            .list_provider_models("anthropic@a1")
            .unwrap()
            .into_iter()
            .map(|m| (m.model_id, m.context_window, m.source))
            .collect();
        assert_eq!(
            own,
            [("model-a".to_string(), Some(200_000), "added".to_string()), ("model-b".to_string(), Some(200_000), "added".to_string())]
        );
        assert!(store.list_provider_models("anthropic").unwrap().is_empty(), "the seeded rows go");
        assert_eq!(store.list_provider_models("openai@o1").unwrap().len(), 1, "a model he added stays as it was");
        assert_eq!(counts, Counts { pinned: 1, kept: 2, removed: 3 });
        assert_eq!(own_models(&store, Some(&owner)), Counts::default(), "nothing left to do");
    }

    /// With no models.yaml of his own, a connection gets what Nebo's
    /// catalog gave it.
    #[test]
    fn with_no_file_of_his_own_a_connection_keeps_what_it_ran() {
        let (_dir, store) = store();
        store.create_auth_profile("a1", "Work", "anthropic", "k", None, None, 10, 1, None, None).unwrap();
        store.create_auth_profile("r1", "Router", "openrouter", "k", None, None, 10, 1, None, None).unwrap();
        own_models(&store, None);
        assert_eq!(store.get_auth_profile("a1").unwrap().unwrap().model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(store.get_auth_profile("r1").unwrap().unwrap().model, None, "no default ever: nothing to write");
    }
}
