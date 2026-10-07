//! The first-party defaults (`tools::default_artifacts`), installed in the
//! background once the bot is signed in to NeboAI: on the desktop after
//! sign-in, on the owner's own server and on a Nebo Cloud bot at boot. Every
//! successful NeboAI connect asks for a pass ([`request`]); one pass runs at
//! a time.
//!
//! A pass installs each default that is not installed and that the owner
//! has not removed, one after another, through the one code door
//! (`codes::handle_code_text`, which claims the code in `InFlightCodes`, so
//! a default some other door is installing is never installed twice). No
//! client asked, so no install surface opens anywhere. A default that did
//! not land is tried again later in the same pass, quietly, with backoff,
//! and logged; the owner never hears about it. A default the owner removed
//! is recorded ([`record_removal`]) and never put back by a pass; a request
//! that needs it still installs it (`default_artifacts::install_on_request`).

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tracing::{info, warn};

use tools::default_artifacts::{DEFAULT_ARTIFACTS, DefaultArtifact, default_artifact};

use crate::state::AppState;

/// The waits between a pass's rounds while a default has not landed. After
/// the last, the pass ends; the next connect starts another.
const RETRY_AFTER: &[Duration] = &[
    Duration::from_secs(30),
    Duration::from_secs(120),
    Duration::from_secs(600),
    Duration::from_secs(1800),
    Duration::from_secs(3600),
];

/// Where the owner's removals are kept: a setting of the NeboAI connection's
/// own registry row, beside the bot's other NeboAI settings.
const SETTINGS_ROW: &str = "neboai";
const REMOVED_KEY: &str = "default_artifacts_removed";

/// Whether a pass is running in this process.
static PASS: AtomicBool = AtomicBool::new(false);

/// Holds the pass flag for the life of one pass, released however it ends.
struct PassGuard;

impl Drop for PassGuard {
    fn drop(&mut self) {
        PASS.store(false, Ordering::SeqCst);
    }
}

/// Ask for a pass over the defaults. Called on every successful NeboAI
/// connect; a pass already running answers it.
pub(crate) fn request(state: &AppState) {
    spawn_pass(state, DEFAULT_ARTIFACTS.to_vec(), RETRY_AFTER);
}

/// Start a pass over `defaults` in the background, unless one is running.
/// Returns the pass, or `None` when one was already running.
pub(crate) fn spawn_pass(
    state: &AppState,
    defaults: Vec<DefaultArtifact>,
    retry_after: &'static [Duration],
) -> Option<tokio::task::JoinHandle<()>> {
    if PASS.swap(true, Ordering::SeqCst) {
        return None;
    }
    let guard = PassGuard;
    let state = state.clone();
    Some(tokio::spawn(async move {
        let _guard = guard;
        let mut waits = retry_after.iter();
        loop {
            let missing = install_missing(&state, &defaults).await;
            if missing.is_empty() {
                return;
            }
            match waits.next() {
                Some(wait) => {
                    info!(
                        ?missing,
                        retry_in_secs = wait.as_secs(),
                        "defaults not installed yet; trying again"
                    );
                    tokio::time::sleep(*wait).await;
                }
                None => {
                    warn!(
                        ?missing,
                        "defaults still not installed; the next NeboAI connect tries again"
                    );
                    return;
                }
            }
        }
    }))
}

/// One round: install each of `defaults` that is not installed and that
/// the owner has not removed. Returns the slugs still missing.
pub(crate) async fn install_missing(
    state: &AppState,
    defaults: &[DefaultArtifact],
) -> Vec<&'static str> {
    let removed = removed(&state.store);
    let mut missing = Vec::new();
    for d in defaults {
        if removed.contains(d.slug) || installed(state, d) {
            continue;
        }
        // Signed out, or another running copy of this bot holds it (a cloud
        // bot replaced mid-pass): nothing is installed from here now.
        if crate::codes::neboai_token(state).is_none() || comm::lease::process().frozen() {
            missing.push(d.slug);
            continue;
        }
        let Some((code_type, code)) = crate::codes::detect_code(d.code) else {
            warn!(
                slug = d.slug,
                code = d.code,
                "a default's code is not an install code"
            );
            continue;
        };
        let outcome =
            crate::codes::handle_code_text(state, code_type, code, tools::InstalledBy::Owner, None)
                .await;
        if installed(state, d) {
            info!(slug = d.slug, "default installed");
        } else {
            warn!(slug = d.slug, %outcome, "default not installed");
            missing.push(d.slug);
        }
    }
    missing
}

/// Is default `d` on this bot? A disabled plugin or skill is: the owner
/// turned it off, which a pass leaves alone.
fn installed(state: &AppState, d: &DefaultArtifact) -> bool {
    if d.is_plugin() {
        state.plugin_store.resolve(d.slug, "*").is_some()
    } else {
        tools::installed::is_installed(d.slug, "", "skill", &state.store)
    }
}

/// The defaults the owner removed.
fn removed(store: &db::Store) -> BTreeSet<String> {
    store
        .get_plugin_setting(SETTINGS_ROW, REMOVED_KEY)
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default()
}

/// The owner removed the plugin or skill `slug`: when it is a default, a
/// pass never puts it back. Called by the removal doors (a plugin's one
/// removal path, a skill's delete, a store uninstall).
pub(crate) fn record_removal(store: &db::Store, slug: &str) {
    let Some(d) = default_artifact(slug) else {
        return;
    };
    let mut removed = removed(store);
    if !removed.insert(d.slug.to_string()) {
        return;
    }
    let value = serde_json::to_string(&removed).unwrap_or_default();
    match store.set_plugin_setting(SETTINGS_ROW, REMOVED_KEY, &value) {
        Ok(()) => info!(
            slug = d.slug,
            "the owner removed a default; it is not reinstalled at startup"
        ),
        Err(e) => warn!(slug = d.slug, error = %e, "the removal of a default was not recorded"),
    }
}

/// Forget the owner's removals (the proof scenarios, between runs).
#[cfg(test)]
pub(crate) fn forget_removals(store: &db::Store) {
    let _ = store.delete_plugin_setting(SETTINGS_ROW, REMOVED_KEY);
}
