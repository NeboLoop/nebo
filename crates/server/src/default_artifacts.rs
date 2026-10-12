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
//!
//! On a Nebo Cloud bot a pass also fetches every installed plugin whose
//! package is not on disk: its saved state leaves packages out
//! (`backup_ship::pack`), so a restored bot has the plugin's records and data
//! and fetches its package again through the one plugin installer
//! (`codes::fetch_and_install_plugin`, what an update runs). And there a pass
//! that has anything to fetch first waits a random spread of up to
//! [`CLOUD_SPREAD`]: a release rolls every cloud bot at once, and their
//! downloads must not land on the hub in one burst. A request that needs a
//! default never waits for it: the safety net installs on the spot.

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

/// The most a cloud bot's pass waits before it fetches anything.
const CLOUD_SPREAD: Duration = Duration::from_secs(600);

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
/// connect; a pass already running answers it. A cloud bot's pass also
/// fetches the plugin packages its restored state left out, and spreads.
pub(crate) fn request(state: &AppState) {
    let cloud = tools::cloud_bot();
    spawn_pass(state, DEFAULT_ARTIFACTS.to_vec(), RETRY_AFTER, cloud, spread(cloud));
}

/// How long a pass that has something to fetch waits first: a random spread
/// of up to [`CLOUD_SPREAD`] on a cloud bot, none on a desktop or the
/// owner's own server.
fn spread(cloud: bool) -> Duration {
    if !cloud {
        return Duration::ZERO;
    }
    use rand::Rng;
    Duration::from_millis(rand::thread_rng().gen_range(0..=CLOUD_SPREAD.as_millis() as u64))
}

/// Start a pass over `defaults` in the background, unless one is running.
/// `packages` also fetches every installed plugin whose package is not on
/// disk (a cloud bot). A pass with something to fetch waits `delay` first;
/// one with nothing to do ends at once. Returns the pass, or `None` when
/// one was already running.
pub(crate) fn spawn_pass(
    state: &AppState,
    defaults: Vec<DefaultArtifact>,
    retry_after: &'static [Duration],
    packages: bool,
    delay: Duration,
) -> Option<tokio::task::JoinHandle<()>> {
    if PASS.swap(true, Ordering::SeqCst) {
        return None;
    }
    let guard = PassGuard;
    let state = state.clone();
    Some(tokio::spawn(async move {
        let _guard = guard;
        if !delay.is_zero() && pending(&state, &defaults, packages) {
            info!(delay_secs = delay.as_secs(), "plugins to fetch; waiting out this bot's spread first");
            tokio::time::sleep(delay).await;
        }
        let mut waits = retry_after.iter();
        loop {
            let mut missing = if packages { fetch_missing_packages(&state).await } else { Vec::new() };
            missing.extend(install_missing(&state, &defaults).await.into_iter().map(str::to_string));
            missing.sort();
            missing.dedup();
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

/// Is there anything for a pass to fetch: a default to install, or (with
/// `packages`) an installed plugin whose package is not on disk?
fn pending(state: &AppState, defaults: &[DefaultArtifact], packages: bool) -> bool {
    let removed = removed(&state.store);
    defaults.iter().any(|d| !removed.contains(d.slug) && !installed(state, d))
        || (packages && !packages_missing(state).is_empty())
}

/// May this process install now? Signed in, and no other running copy of
/// this bot holds it (a cloud bot replaced mid-pass).
fn may_install(state: &AppState) -> bool {
    crate::codes::neboai_token(state).is_some() && !comm::lease::process().frozen()
}

/// The installed plugins (the registry's rows) with no package on disk.
fn packages_missing(state: &AppState) -> Vec<db::models::PluginRegistry> {
    match state.store.list_installed_plugins() {
        Ok(rows) => rows.into_iter().filter(|p| !state.plugin_store.package_on_disk(&p.slug)).collect(),
        Err(e) => {
            warn!(error = %e, "the installed plugins could not be read");
            Vec::new()
        }
    }
}

/// One round: fetch the package of each installed plugin that has none on
/// disk — what a restored cloud bot had — through the one plugin installer,
/// the one an update runs; its data and settings were restored and stay.
/// A default's code is claimed, so a request that needs it waits for this
/// install instead of fetching it a second time. Returns the slugs still
/// missing.
async fn fetch_missing_packages(state: &AppState) -> Vec<String> {
    let mut missing = Vec::new();
    for p in packages_missing(state) {
        if !may_install(state) {
            missing.push(p.slug);
            continue;
        }
        let code = default_artifact(&p.slug).filter(|d| d.is_plugin()).map(|d| d.code);
        let claim = match code {
            Some(code) => match state.codes_in_flight.begin(code) {
                Some(claim) => Some(claim),
                // Another door is installing it now.
                None => {
                    missing.push(p.slug);
                    continue;
                }
            },
            None => None,
        };
        let name = if p.display_name.is_empty() { p.slug.clone() } else { p.display_name.clone() };
        let fetched = match crate::codes::build_api_client(state) {
            Ok(api) => crate::codes::fetch_and_install_plugin(state, &api, &p.slug, &name, None)
                .await
                .map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        drop(claim);
        match fetched {
            Ok(()) => info!(slug = %p.slug, "plugin package fetched for a restored bot"),
            Err(e) => {
                warn!(slug = %p.slug, error = %e, "plugin package not fetched");
                missing.push(p.slug);
            }
        }
    }
    missing
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
        if !may_install(state) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A cloud bot's pass waits a random spread within ten minutes, and not
    /// the same one every time; a desktop's, or the owner's own server's,
    /// waits none.
    #[test]
    fn a_cloud_bot_spreads_its_pass_and_a_desktop_does_not() {
        let waits: Vec<Duration> = (0..200).map(|_| spread(true)).collect();
        assert!(waits.iter().all(|w| *w <= CLOUD_SPREAD), "{waits:?}");
        assert_eq!(CLOUD_SPREAD, Duration::from_secs(600));
        let distinct: std::collections::BTreeSet<Duration> = waits.iter().copied().collect();
        assert!(distinct.len() > 100, "the spread is random: {} distinct of 200", distinct.len());
        assert!(waits.iter().any(|w| *w < CLOUD_SPREAD / 2) && waits.iter().any(|w| *w > CLOUD_SPREAD / 2));
        assert!((0..50).all(|_| spread(false).is_zero()), "a desktop never waits");
    }
}
