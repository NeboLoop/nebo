//! What "Hire from another app" offers, known before it is asked: the apps
//! installed on this computer and, on each of the owner's linked computers,
//! its apps and the agents they run. Looking for them means asking the hub
//! for the owner's bots and each linked bot for its roster, which takes
//! seconds; the hire list answers from what was last found instead, at once
//! ([`LinkedApps::current_or_look`]).
//!
//! It is looked for again at startup, every [`EVERY`], each time Nebo
//! connects to NeboAI, and behind every listing. When what the hire list
//! shows changes, ONE `linked_apps_changed` event carries the new list to
//! every open list.

use std::time::Duration;

use tokio::sync::{Mutex, RwLock};
use tracing::info;

use crate::state::AppState;

/// How often the owner's computers are looked at again while Nebo runs.
pub const EVERY: Duration = Duration::from_secs(5 * 60);

/// What was found on the owner's computers.
#[derive(Clone, Default)]
pub struct Inventory {
    /// Each linked bot the owner can hire from, with its roster.
    pub links: Vec<(comm::api_types::ManagedBot, comm::api_types::LinkedRoster)>,
    /// The coding agents installed on this computer that Nebo can host.
    pub installed: Vec<comm::api_types::LinkedRuntime>,
    /// When it was found, in unix seconds.
    pub checked_at: i64,
}

/// The inventory, and the one look for it at a time.
#[derive(Default)]
pub struct LinkedApps {
    known: RwLock<Option<Inventory>>,
    looking: Mutex<()>,
}

impl LinkedApps {
    /// What was last found, and a look for anything newer behind it; the
    /// very first time, before anything was found, the look itself.
    pub async fn current_or_look(&self, state: &AppState) -> Inventory {
        if let Some(known) = self.known.read().await.clone() {
            let state = state.clone();
            tokio::spawn(async move { state.linked_apps.look(&state).await });
            return known;
        }
        self.look(state).await;
        self.known.read().await.clone().unwrap_or_default()
    }

    /// Looks at the owner's computers again and keeps what is found. When
    /// the hire list it makes differs from the one before, every open list
    /// is sent the new one (`linked_apps_changed`). A look already under way
    /// is this look: it is waited for, not repeated. The hub not answering
    /// keeps the linked computers last found.
    pub async fn look(&self, state: &AppState) {
        let Ok(_one) = self.looking.try_lock() else {
            let _wait = self.looking.lock().await;
            return;
        };
        let before = self.known.read().await.clone();
        let mut found = find(state).await;
        let links = match (found.links.take(), &before) {
            (Some(links), _) => links,
            (None, Some(before)) => before.links.clone(),
            (None, None) => Vec::new(),
        };
        let inventory = Inventory { links, installed: found.installed, checked_at: chrono::Utc::now().timestamp() };
        let listed = crate::handlers::agents::linked_listing(state, &inventory);
        let changed = before.as_ref().is_some_and(|b| crate::handlers::agents::linked_listing(state, b) != listed);
        *self.known.write().await = Some(inventory.clone());
        if changed {
            info!(computers = listed.len(), "linked: the apps on the owner's computers changed");
            state.hub.broadcast(
                "linked_apps_changed",
                serde_json::json!({ "computers": listed, "checkedAt": inventory.checked_at }),
            );
        }
    }
}

/// One look's findings: the linked bots and their rosters (`None` when the
/// hub could not be asked), and what is installed here.
struct Found {
    links: Option<Vec<(comm::api_types::ManagedBot, comm::api_types::LinkedRoster)>>,
    installed: Vec<comm::api_types::LinkedRuntime>,
}

/// Looks at this computer and asks the hub for the owner's linked bots, then
/// each for its roster. Not signed in to NeboAI = no linked computers.
async fn find(state: &AppState) -> Found {
    let installed = state
        .local_host
        .as_ref()
        .filter(|local| local.bot_id().is_some())
        .map(|local| {
            local
                .hireable()
                .into_iter()
                .map(|a| comm::api_types::LinkedRuntime { id: a.id, name: a.name })
                .collect()
        })
        .unwrap_or_default();
    let Ok(api) = crate::codes::build_api_client(state) else {
        return Found { links: Some(Vec::new()), installed };
    };
    let bots = match api.list_managed_bots().await {
        Ok(bots) => bots,
        Err(e) => {
            info!(error = %e, "linked: the owner's bots could not be listed");
            return Found { links: None, installed };
        }
    };
    let self_id = config::read_bot_id().unwrap_or_default();
    let sources: Vec<comm::api_types::ManagedBot> =
        bots.into_iter().filter(|b| crate::handlers::agents::hire_source(b, &self_id)).collect();
    let rosters = futures::future::join_all(sources.iter().map(|bot| api.linked_bot_roster(&bot.id))).await;
    let links = sources
        .into_iter()
        .zip(rosters)
        .map(|(bot, roster)| {
            let roster = roster.unwrap_or_else(|e| {
                info!(bot_id = %bot.id, error = %e, "linked bot's roster did not answer");
                comm::api_types::LinkedRoster::default()
            });
            (bot, roster)
        })
        .collect();
    Found { links: Some(links), installed }
}

/// Looks at startup, then every [`EVERY`], for as long as Nebo runs.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut every = tokio::time::interval(EVERY);
        loop {
            every.tick().await;
            state.linked_apps.look(&state).await;
        }
    });
}
