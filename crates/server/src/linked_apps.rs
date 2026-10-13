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
//!
//! Each look also hosts, here, every employee whose coding agent works on
//! this computer though another bot recorded it ([`host_here`]).

use std::time::Duration;

use std::path::{Path, PathBuf};

use nebo_runtimes::acp::Agent as AcpAgent;
use tokio::sync::{Mutex, RwLock};
use tracing::{info, warn};

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
        host_here(state, &links).await;
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

/// Hosts, on this computer, every employee whose coding agent works here
/// under another bot (a nebo-link of this computer's that recorded it): an
/// employee whose agent (Claude Code, Codex, Gemini CLI, OpenCode) is
/// installed here and whose folder is on this computer becomes Nebo's own,
/// through the hire path ([`ai::LocalHost::hire`]). It stays the same
/// employee, with its id, name and chats; its agent keeps its id, kind and
/// folder, and each chat keeps the agent's session it is bound to, which its
/// next message reopens from this computer. Only while Nebo hosts this
/// computer's agents (no nebo-link daemon linked for this user), and never
/// one whose bot is online (`links`: its host still runs it). Returns the
/// employees hosted here now.
pub(crate) async fn host_here(
    state: &AppState,
    links: &[(comm::api_types::ManagedBot, comm::api_types::LinkedRoster)],
) -> Vec<String> {
    let Some(local) = state.local_host.clone() else {
        return Vec::new();
    };
    let (Some(here), Some(_)) = (local.bot_id(), local.oal()) else {
        return Vec::new();
    };
    let installed: Vec<String> = local.hireable().into_iter().map(|a| a.id).collect();
    let rows = state.store.list_agents(1000, 0).unwrap_or_default();
    let linked = crate::handlers::agents::linked_employees(state, &rows);
    let mut hosted = Vec::new();
    for (employee, brain) in &linked {
        let Some((bot, agent)) = ai::LinkedProvider::target(brain) else {
            continue;
        };
        if bot == here || links.iter().any(|(b, _)| b.id == bot && b.online) {
            continue;
        }
        let Some((kind, label)) = coding_agent(agent) else {
            continue;
        };
        if !installed.iter().any(|k| k == kind.key()) {
            continue;
        }
        let Some(folder) = folder(state, &local, employee, agent) else {
            continue;
        };
        match host(state, &local, &here, &linked, employee, agent, kind, &label, &folder).await {
            Ok(id) => {
                info!(employee = %employee, from_bot = bot, agent = %id, folder = %folder.display(), "linked: Nebo hosts the employee's coding agent on this computer");
                let name = rows.iter().find(|r| &r.id == employee).map(|r| r.name.clone()).unwrap_or_default();
                state.hub.broadcast("agent_updated", serde_json::json!({ "agentId": employee, "name": name }));
                hosted.push(employee.clone());
            }
            Err(e) => warn!(employee = %employee, from_bot = bot, agent, error = %e, "linked: the employee's coding agent could not be hosted on this computer"),
        }
    }
    hosted
}

/// The coding agent a linked agent's id names, and the name it was given:
/// a host names each after its agent ("Claude Code", "Claude Code 2",
/// "Codex · api") and makes its id from that name ("claude-code-2").
fn coding_agent(id: &str) -> Option<(AcpAgent, String)> {
    AcpAgent::KNOWN.into_iter().find_map(|kind| {
        let rest = id.strip_prefix(&link_core::roster::agent_id(kind.name()))?;
        let label = match rest {
            "" => kind.name().to_owned(),
            rest => format!("{} {}", kind.name(), rest.strip_prefix('-')?.replace('-', " ")),
        };
        (link_core::roster::agent_id(&label) == id).then_some((kind, label))
    })
}

/// The folder the employee's agent `agent` works in, when it is on this
/// computer: where its latest conversation with it works, else the agent's
/// own (`~/NeboAI/<agent>`).
fn folder(state: &AppState, local: &ai::LocalHost, employee: &str, agent: &str) -> Option<PathBuf> {
    let chats = state.store.list_recent_agent_chats(employee, 50).unwrap_or_default();
    let worked = chats
        .into_iter()
        .map(|(chat, _)| chat)
        .filter(|chat| chat.linked_agent_id.as_deref() == Some(agent))
        .find_map(|chat| chat.linked_folder)
        .map(PathBuf::from);
    let folder = worked.unwrap_or_else(|| local.own_folder(agent));
    folder.is_dir().then(|| folder.canonicalize().unwrap_or(folder))
}

/// Hosts the employee's agent here in `folder` and makes it the employee's
/// brain: the agent Nebo already hosts there for nobody else, else a new
/// one, hired as `label` so it keeps its id. Each of the employee's chats
/// stays bound to its session; under another id, they follow it. Returns
/// the id Nebo hosts it as.
#[allow(clippy::too_many_arguments)]
async fn host(
    state: &AppState,
    local: &ai::LocalHost,
    here: &str,
    linked: &[(String, String)],
    employee: &str,
    agent: &str,
    kind: AcpAgent,
    label: &str,
    folder: &Path,
) -> Result<String, String> {
    let brain_of = |id: &str| ai::LinkedProvider::model_id(here, id);
    let existing = local.agents().into_iter().find(|a| a.agent == kind && a.acp.workdir == folder);
    let id = match existing {
        Some(a) if linked.iter().any(|(e, brain)| e != employee && *brain == brain_of(&a.id)) => {
            return Err(format!("{} already works in {} for another employee", a.label, folder.display()));
        }
        Some(a) => a.id,
        None => local.hire(kind.key(), Some(label), Some(folder)).await?.id,
    };
    if id != agent {
        state.store.relink_agent_chats(employee, agent, &id).map_err(|e| e.to_string())?;
    }
    state
        .store
        .upsert_entity_config("agent", employee, &serde_json::json!({ "modelPreference": brain_of(&id) }))
        .map_err(|e| e.to_string())?;
    Ok(id)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A host names each coding agent after what it is and makes its id
    /// from that name: the id says which agent it is and what it was named.
    #[test]
    fn a_coding_agents_id_names_its_agent_and_its_name() {
        let named = |id: &str| coding_agent(id).map(|(kind, label)| (kind.key(), label));
        assert_eq!(named("claude-code"), Some(("claude-code", "Claude Code".to_owned())));
        assert_eq!(named("claude-code-5"), Some(("claude-code", "Claude Code 5".to_owned())));
        assert_eq!(named("codex-2"), Some(("codex", "Codex 2".to_owned())));
        assert_eq!(named("gemini-cli"), Some(("gemini", "Gemini CLI".to_owned())));
        assert_eq!(named("opencode-api"), Some(("opencode", "OpenCode api".to_owned())));
        assert_eq!(named("claude-coder"), None);
        assert_eq!(named("hermes"), None);
        assert_eq!(named("assistant"), None);
    }
}
