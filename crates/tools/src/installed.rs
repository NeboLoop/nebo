//! Is this marketplace artifact installed here? ONE answer, shared by the
//! store page (`server::handlers::store`) and the tools that offer install
//! cards in chat. It lived in the store handler; the agent tool needed the
//! same answer and `crates/tools` cannot depend on `crates/server`, so it
//! moved down to where both can reach it — two implementations of "is it
//! installed" would drift one forgotten case at a time (CODE_AUDITOR §8.1).

/// Check if an artifact is installed locally by slug, checking both
/// user and nebo artifact directories.
///
/// All artifact types (skills, agents, plugins) use filesystem-based
/// discovery. The DB stores mutable state (enabled, input_values) but
/// is NOT the source of truth for installation.
pub fn is_locally_installed(slug: &str, artifact_type: &str) -> bool {
    let (user_dir, nebo_dir) = match (config::user_dir(), config::nebo_dir()) {
        (Ok(u), Ok(n)) => (u, n),
        _ => return false,
    };

    // Check user dir
    let user_path = user_dir.join(artifact_type).join(slug);
    if user_path.exists() {
        return true;
    }

    // Check nebo (marketplace) dir
    let nebo_path = nebo_dir.join(artifact_type).join(slug);
    nebo_path.exists()
}

/// Check if a product is installed on the filesystem.
pub fn is_installed(slug: &str, _name: &str, artifact_type: &str, _store: &db::Store) -> bool {
    let dir_type = match artifact_type {
        "agent" => "agents",
        "skill" => "skills",
        "plugin" => "plugins",
        _ => "skills",
    };
    if dir_type == "agents" {
        // Truthful check via the loader's own discovery criteria: the dir must
        // contain something the agent loader actually loads (an AGENT.md tree
        // or a sealed .napp). A bare directory left behind by a failed install
        // used to satisfy the plain `exists()` check and show "Installed" for
        // an agent that never materialized.
        let (Ok(user_dir), Ok(nebo_dir)) = (config::user_dir(), config::nebo_dir()) else {
            return false;
        };
        return napp::agent_loader::dir_contains_agent(&user_dir.join("agents").join(slug))
            || napp::agent_loader::dir_contains_agent(&nebo_dir.join("agents").join(slug));
    }
    is_locally_installed(slug, dir_type)
}

/// Local install state snapshotted ONCE per response. Enriching a 100-item
/// page by calling `is_installed` per product meant 2 config-dir resolutions
/// and several path stats per item — 5-8s per page on cloud bots, where every
/// virtio-fs stat is a guest→host round trip. Four read_dirs + one DB query
/// up front turn per-item enrichment into hash lookups.
pub struct InstalledIndex {
    agents: std::collections::HashSet<String>,
    skills: std::collections::HashSet<String>,
    plugins: std::collections::HashSet<String>,
    updates: Vec<db::models::ArtifactUpdatePref>,
}

impl InstalledIndex {
    pub fn build(store: &db::Store) -> Self {
        let mut idx = InstalledIndex {
            agents: std::collections::HashSet::new(),
            skills: std::collections::HashSet::new(),
            plugins: std::collections::HashSet::new(),
            updates: store.list_artifacts_with_updates().unwrap_or_default(),
        };
        let (Ok(user_dir), Ok(nebo_dir)) = (config::user_dir(), config::nebo_dir()) else {
            return idx;
        };
        for root in [&user_dir, &nebo_dir] {
            // Same truthful criterion as `is_installed`: an agent dir counts
            // only if the loader would actually load it.
            if let Ok(entries) = std::fs::read_dir(root.join("agents")) {
                for e in entries.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if !idx.agents.contains(&name)
                        && napp::agent_loader::dir_contains_agent(&e.path())
                    {
                        idx.agents.insert(name);
                    }
                }
            }
            for (dir, set) in [("skills", &mut idx.skills), ("plugins", &mut idx.plugins)] {
                if let Ok(entries) = std::fs::read_dir(root.join(dir)) {
                    for e in entries.flatten() {
                        set.insert(e.file_name().to_string_lossy().into_owned());
                    }
                }
            }
        }
        idx
    }

    /// Mirrors `is_installed`'s type mapping: unknown types check skills.
    pub fn contains(&self, slug: &str, artifact_type: &str) -> bool {
        match artifact_type {
            "agent" => self.agents.contains(slug),
            "plugin" => self.plugins.contains(slug),
            _ => self.skills.contains(slug),
        }
    }
}

/// Enrich a single product JSON value with local install state and update availability.
pub fn enrich_installed_item(val: &mut serde_json::Value, idx: &InstalledIndex) {
    if let Some(obj) = val.as_object_mut() {
        let slug = obj.get("slug").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let artifact_type = obj.get("type").and_then(|v| v.as_str()).unwrap_or("skill").to_string();
        let artifact_id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if !slug.is_empty() && idx.contains(&slug, &artifact_type) {
            obj.insert("installed".to_string(), serde_json::Value::Bool(true));

            // Check if an update is available for this artifact
            let lookup_id = if artifact_id.is_empty() { &slug } else { &artifact_id };
            if let Some(pref) = idx
                .updates
                .iter()
                .find(|p| p.artifact_id == *lookup_id || p.artifact_id == slug)
            {
                obj.insert("updateAvailable".to_string(), serde_json::Value::Bool(true));
                obj.insert(
                    "remoteVersion".to_string(),
                    serde_json::Value::String(pref.remote_version.clone()),
                );
                // The id apply-update keys on (pref.artifact_id: slug for plugins,
                // marketplace UUID for skills/agents) differs from the marketplace
                // UUID this page is routed by. Hand the frontend the exact key so the
                // apply call hits the same record this badge was derived from.
                obj.insert(
                    "updateId".to_string(),
                    serde_json::Value::String(pref.artifact_id.clone()),
                );
            }
        }
    }
}

/// Enrich a product list response with local install state.
/// Looks for `{ "skills": [...] }` structure.
pub fn enrich_installed_state(resp: &mut serde_json::Value, store: &db::Store) {
    if let Some(items) = resp.get_mut("products").and_then(|v| v.as_array_mut()) {
        if items.is_empty() {
            return;
        }
        let idx = InstalledIndex::build(store);
        for item in items.iter_mut() {
            enrich_installed_item(item, &idx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The store page and the hire card must agree on what "installed" means:
    // an agent is found by the agents set, a plugin by the plugins set, and
    // anything else (skill, workflow, unknown) by the skills set.
    #[test]
    fn contains_maps_type_to_the_right_set() {
        let idx = InstalledIndex {
            agents: ["receptionist".to_string()].into_iter().collect(),
            skills: ["invoicing".to_string()].into_iter().collect(),
            plugins: ["gmail".to_string()].into_iter().collect(),
            updates: vec![],
        };
        assert!(idx.contains("receptionist", "agent"));
        assert!(!idx.contains("receptionist", "plugin"));
        assert!(idx.contains("gmail", "plugin"));
        assert!(idx.contains("invoicing", "skill"));
        assert!(idx.contains("invoicing", "workflow"), "unknown types check skills");
        assert!(!idx.contains("gmail", "agent"));
    }

    // Enrichment only ever adds `installed: true`; it never writes false, so a
    // listing the index does not know stays untouched rather than being
    // labelled uninstalled on a guess.
    #[test]
    fn enrichment_marks_installed_and_leaves_others_alone() {
        let idx = InstalledIndex {
            agents: ["receptionist".to_string()].into_iter().collect(),
            skills: Default::default(),
            plugins: Default::default(),
            updates: vec![],
        };
        let mut hit = serde_json::json!({"slug": "receptionist", "type": "agent", "id": "x"});
        let mut miss = serde_json::json!({"slug": "dispatcher", "type": "agent", "id": "y"});
        enrich_installed_item(&mut hit, &idx);
        enrich_installed_item(&mut miss, &idx);
        assert_eq!(hit["installed"], serde_json::Value::Bool(true));
        assert!(miss.get("installed").is_none());
    }
}
