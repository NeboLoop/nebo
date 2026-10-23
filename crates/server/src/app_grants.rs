//! What an app may do beyond its own page and store: the permissions its
//! manifest asks for, as the owner decided them.
//!
//! Reading his files (`filesystem:`), talking to another website
//! (`network:`), asking another of his employees (`subagent:`) and running
//! one of his plugins (`plugin:`) need his OK
//! (`napp::manifest::needs_consent`):
//!
//! - An app his own employees build on this Nebo (`create_employee`,
//!   `update_employee`: `tools::app_dev::is_own_app`) holds what its
//!   manifest asks for.
//! - An app installed from the marketplace or shared through a loop holds
//!   none of them until he allows each one, in plain words, when he opens
//!   it. He can take any back in the app's settings. An update that asks
//!   for something new is asked about again: nothing he answered covers it.
//!
//! Every check (`handlers::apps`: the web proxy, invoking an employee, the
//! file route; `handlers::app_plugins`: running a plugin) reads
//! [`granted`], and refuses what it does not cover.

use crate::handlers::permissions::{AppPermission, AppPermissions};
use crate::state::AppState;

/// What the app asks for and whose it is.
pub(crate) struct Asked {
    pub(crate) permissions: Vec<String>,
    own: bool,
}

/// What `app_id` asks for, from its manifest as the loader holds it; `None`
/// for an id that names no app.
pub(crate) async fn asked(state: &AppState, app_id: &str) -> Option<Asked> {
    let row = state.store.get_agent(app_id).ok().flatten().filter(|a| a.is_app.unwrap_or(0) != 0)?;
    let permissions = crate::handlers::apps::loaded_app(state, app_id)
        .await
        .map(|a| a.app_permissions)
        .unwrap_or_default();
    Some(Asked { permissions, own: tools::app_dev::is_own_app(&row) })
}

/// The permissions `asked` holds, given the owner's answers: everything for
/// his own app; for any other, what needs no OK and what he allowed.
fn held(asked: &Asked, answers: &[(String, bool)]) -> Vec<String> {
    asked
        .permissions
        .iter()
        .filter(|p| asked.own || !napp::manifest::needs_consent(p) || answers.iter().any(|(a, ok)| a == *p && *ok))
        .cloned()
        .collect()
}

/// The permissions app `app_id` holds now. Empty for an id that names no
/// app.
pub(crate) async fn granted(state: &AppState, app_id: &str) -> Vec<String> {
    let Some(asked) = asked(state, app_id).await else { return Vec::new() };
    granted_of(state, app_id, &asked)
}

/// The permissions `asked` (app `app_id`'s requests) holds now.
pub(crate) fn granted_of(state: &AppState, app_id: &str, asked: &Asked) -> Vec<String> {
    let answers = state.store.app_permission_answers(app_id).unwrap_or_default();
    held(asked, &answers)
}

/// What a request's target is called: an employee's name for a `subagent:`
/// one, a plugin's for a `plugin:` one.
pub(crate) trait Names {
    fn employee(&self, id: &str) -> Option<String>;
    fn plugin(&self, slug: &str) -> Option<String>;
}

/// [`Names`] from this Nebo's store and installed plugins.
struct Installed<'a>(&'a AppState);

impl Names for Installed<'_> {
    fn employee(&self, id: &str) -> Option<String> {
        self.0.store.get_agent(id).ok().flatten().map(|a| a.name)
    }

    fn plugin(&self, slug: &str) -> Option<String> {
        self.0.plugin_store.get_manifest(slug).map(|m| m.name).filter(|n| !n.is_empty())
    }
}

/// [`AppPermissions`] for an app, its targets named by `names`.
pub(crate) fn listing(asked: &Asked, answers: &[(String, bool)], names: &impl Names) -> AppPermissions {
    let permissions = asked
        .permissions
        .iter()
        .filter(|p| napp::manifest::needs_consent(p))
        .map(|p| {
            let (kind, target) = describe(p, names);
            let state = if asked.own {
                "allowed"
            } else {
                match answers.iter().find(|(a, _)| a == p) {
                    Some((_, true)) => "allowed",
                    Some((_, false)) => "refused",
                    None => "asking",
                }
            };
            AppPermission { permission: p.clone(), kind: kind.into(), target, state: state.into() }
        })
        .collect();
    AppPermissions { own: asked.own, permissions }
}

/// [`AppPermissions`] for app `app_id`, names resolved from the store.
pub(crate) async fn listing_for(state: &AppState, app_id: &str) -> Result<AppPermissions, types::NeboError> {
    let asked = asked(state, app_id).await.ok_or(types::NeboError::NotFound)?;
    let answers = state.store.app_permission_answers(app_id)?;
    Ok(listing(&asked, &answers, &Installed(state)))
}

/// Record the owner's answer to one of app `app_id`'s requests.
pub(crate) async fn answer(state: &AppState, app_id: &str, permission: &str, allowed: bool) -> Result<(), types::NeboError> {
    let asked = asked(state, app_id).await.ok_or(types::NeboError::NotFound)?;
    if !answerable(&asked, permission) {
        return Err(types::NeboError::Validation(format!(
            "{permission} is not something this app asks you for"
        )));
    }
    state.store.answer_app_permission(app_id, permission, allowed)
}

/// Whether `permission` is one `asked` needs the owner's answer for.
pub(crate) fn answerable(asked: &Asked, permission: &str) -> bool {
    !asked.own && napp::manifest::needs_consent(permission) && asked.permissions.iter().any(|p| p == permission)
}

/// What a permission is about, in the owner's words.
fn describe(perm: &str, names: &impl Names) -> (&'static str, String) {
    let any = |scope: &str| scope.is_empty() || scope == "*";
    if let Some(scope) = perm.strip_prefix("filesystem:") {
        let folder = napp::manifest::file_read_folder(perm).unwrap_or("");
        let _ = scope;
        let mut target = String::from("NeboAI");
        for part in folder.split('/').filter(|p| !p.is_empty()) {
            target.push_str(" › ");
            target.push_str(part);
        }
        return ("files", target);
    }
    if let Some(scope) = perm.strip_prefix("network:") {
        return ("web", if any(scope) { String::new() } else { scope.to_string() });
    }
    if let Some((slug, command)) = napp::manifest::plugin_grant(perm) {
        let mut target = names.plugin(slug).unwrap_or_else(|| slug.to_string());
        if let Some(command) = command {
            target.push_str(" › ");
            target.push_str(command);
        }
        return ("plugin", target);
    }
    let scope = perm.strip_prefix("subagent:").unwrap_or("");
    ("helper", if any(scope) { String::new() } else { names.employee(scope).unwrap_or_else(|| scope.to_string()) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asked(own: bool) -> Asked {
        Asked {
            permissions: [
                "storage:readwrite",
                "filesystem:read:Media/Projects",
                "network:api.example.com",
                "network:*",
                "subagent:emp-1",
                "device:motion",
                "plugin:nebo-media",
                "plugin:nebo-media:strip",
            ]
                .map(String::from)
                .to_vec(),
            own,
        }
    }

    /// The owner's own app holds what it asks for; an installed one holds
    /// only what needs no OK until he allows each of the rest.
    #[test]
    fn an_installed_app_holds_nothing_he_has_not_allowed() {
        let mine = held(&asked(true), &[]);
        assert_eq!(mine.len(), 8);
        let theirs = held(&asked(false), &[]);
        assert_eq!(theirs, vec!["storage:readwrite", "device:motion"]);
        let answers = [("filesystem:read:Media/Projects".to_string(), true), ("network:*".to_string(), false)];
        let theirs = held(&asked(false), &answers);
        assert!(theirs.contains(&"filesystem:read:Media/Projects".to_string()));
        assert!(!napp::manifest::permission_covers(&theirs, "network:evil.example"), "refused stays refused");
        assert!(!napp::manifest::permission_covers(&theirs, "subagent:emp-1"), "unanswered is not held");
        assert!(!napp::manifest::grants_plugin_command(&theirs, "nebo-media", Some("strip")), "an unanswered plugin is not held");
        let plugin = [("plugin:nebo-media:strip".to_string(), true)];
        let theirs = held(&asked(false), &plugin);
        assert!(napp::manifest::grants_plugin_command(&theirs, "nebo-media", Some("strip")));
        assert!(!napp::manifest::grants_plugin_command(&theirs, "nebo-media", Some("render")), "only the command he allowed");
        // An answer for something the manifest no longer asks for holds nothing.
        let stale = [("network:old.example".to_string(), true)];
        assert!(!napp::manifest::permission_covers(&held(&asked(false), &stale), "network:old.example"));
    }

    #[test]
    fn the_listing_says_each_request_in_his_words() {
        struct Fixed;
        impl Names for Fixed {
            fn employee(&self, id: &str) -> Option<String> {
                (id == "emp-1").then(|| "Bookkeeper".to_string())
            }
            fn plugin(&self, slug: &str) -> Option<String> {
                (slug == "nebo-media").then(|| "Nebo Media".to_string())
            }
        }
        let names = &Fixed;
        let l = listing(&asked(false), &[("network:api.example.com".into(), false)], names);
        assert!(!l.own);
        let by = |p: &str| l.permissions.iter().find(|x| x.permission == p).unwrap().clone();
        assert_eq!(l.permissions.len(), 6, "storage and motion need no OK");
        assert_eq!((by("plugin:nebo-media").kind.as_str(), by("plugin:nebo-media").target.as_str()), ("plugin", "Nebo Media"));
        assert_eq!(by("plugin:nebo-media:strip").target, "Nebo Media › strip");
        assert!(answerable(&asked(false), "plugin:nebo-media"));
        assert_eq!((by("filesystem:read:Media/Projects").kind.as_str(), by("filesystem:read:Media/Projects").target.as_str()), ("files", "NeboAI › Media › Projects"));
        assert_eq!(by("filesystem:read:Media/Projects").state, "asking");
        assert_eq!((by("network:api.example.com").target.as_str(), by("network:api.example.com").state.as_str()), ("api.example.com", "refused"));
        assert_eq!(by("network:*").target, "");
        assert_eq!((by("subagent:emp-1").kind.as_str(), by("subagent:emp-1").target.as_str()), ("helper", "Bookkeeper"));
        assert!(listing(&asked(true), &[], names).permissions.iter().all(|p| p.state == "allowed"));
        assert!(answerable(&asked(false), "network:*") && !answerable(&asked(true), "network:*"));
        assert!(!answerable(&asked(false), "storage:readwrite") && !answerable(&asked(false), "network:other.example"));
    }
}
