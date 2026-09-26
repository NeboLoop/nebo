//! This computer's coding agents, hosted by Nebo itself. Where Nebo runs,
//! Nebo is the host: a Claude Code, Codex, Gemini CLI or OpenCode the owner
//! hires on this computer runs as Nebo's own child process, driven through
//! link-core, with no nebo-link daemon and nothing sent through the hub.
//! Its employee's brain is `linked/<this bot>/<agent>` like any linked
//! agent's; [`super::linked::LinkedProvider`] reaches it here instead of
//! through the hub, over the same chat contract.
//!
//! One host per computer per OS user ([`link_core::machine`]): while a
//! nebo-link daemon is linked for this user, it is the host, Nebo hosts
//! nothing, and this computer's agents are hired from the daemon's bot like
//! any other computer's.
//!
//! Every agent works in its own folder, `~/NeboAI/<agent id>`. Nebo's
//! record, under `<data dir>/link/`:
//!
//! ```text
//! agents.json                 every hosted agent: its id, name, which agent
//!                             it is, how it starts, its folder
//! agents/<id>/acp-chats.json  the chats an agent that can't list its
//!                             sessions was given
//! logs/<id>.log               each agent's own output
//! ```

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use link_core::acp::{Acp, AcpLink, Client, Settings};
use link_core::host::Host;
use link_core::phone::Contract;
use link_core::roster::{Member, Roster};
use nebo_runtimes::acp::Agent as AcpAgent;
use nebo_runtimes::{Environment, Runtime, RuntimeCommand};
use serde::{Deserialize, Serialize};
use tracing::info;

/// Who drives the agents, as ACP's `initialize` introduces Nebo.
const CLIENT: Client = Client {
    name: "nebo",
    version: env!("CARGO_PKG_VERSION"),
};

/// One coding agent Nebo hosts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalAgent {
    /// Its id, fixed when it is hired: its employee names it.
    pub id: String,
    /// Its name: "Claude Code", "Claude Code 2".
    pub label: String,
    pub agent: AcpAgent,
    /// How it starts, and the folder it works in.
    pub acp: AcpLink,
}

/// This Nebo's bot id, read per call (a bot gets one when it is first
/// linked to NeboAI). `None` = none yet, so nothing is hosted as it.
pub type BotIdSource = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// A coding agent installed on this computer that Nebo can host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hireable {
    /// `claude-code`, `codex`, `gemini`, `opencode`.
    pub key: &'static str,
    pub name: &'static str,
}

/// This computer's host.
pub struct LocalHost {
    /// This computer's bot: Nebo's own.
    bot_id: BotIdSource,
    /// `<data dir>/link`.
    dir: PathBuf,
    /// The OS user's home, where `~/NeboAI` is.
    home: PathBuf,
    /// Where this OS user's nebo-link daemon would keep its state.
    daemon_home: Option<PathBuf>,
    contract: Arc<Contract>,
    agents: Mutex<Vec<LocalAgent>>,
    /// One hire at a time, so two never take the same id or folder.
    hiring: tokio::sync::Mutex<()>,
}

impl LocalHost {
    /// The host for Nebo's bot, keeping its record in `dir`. `daemon_home`
    /// is where a nebo-link daemon of this OS user keeps its state
    /// ([`link_core::machine::daemon_home`]).
    pub fn open(
        bot_id: BotIdSource,
        dir: PathBuf,
        home: PathBuf,
        daemon_home: Option<PathBuf>,
    ) -> Arc<Self> {
        let agents: Vec<LocalAgent> = std::fs::read_to_string(dir.join("agents.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let members = agents.iter().map(|a| member(&dir, a)).collect();
        let host = Host::new(Arc::new(Roster::new(members)));
        let contract = Contract::new(AcpAgent::Other.key(), AcpAgent::Other.name(), host, None);
        Arc::new(Self {
            bot_id,
            dir,
            home,
            daemon_home,
            contract,
            agents: Mutex::new(agents),
            hiring: tokio::sync::Mutex::new(()),
        })
    }

    /// This computer's bot id: the bot a local employee's brain names.
    pub fn bot_id(&self) -> Option<String> {
        (self.bot_id)()
    }

    /// The nebo-link bot hosting this computer's agents instead of Nebo,
    /// when one is linked for this OS user.
    pub fn hosted_by_daemon(&self) -> Option<String> {
        self.daemon_home
            .as_deref()
            .and_then(link_core::machine::linked_daemon)
    }

    /// The chat contract the hosted agents are reached through; `None`
    /// while a nebo-link daemon is this computer's host.
    pub fn contract(&self) -> Option<Arc<Contract>> {
        self.hosted_by_daemon()
            .is_none()
            .then(|| self.contract.clone())
    }

    /// Every agent Nebo hosts.
    pub fn agents(&self) -> Vec<LocalAgent> {
        self.agents.lock().expect("local agents").clone()
    }

    /// The coding agents installed on this computer that Nebo can host;
    /// none while a nebo-link daemon is the host.
    pub fn hireable(&self) -> Vec<Hireable> {
        if self.hosted_by_daemon().is_some() {
            return Vec::new();
        }
        let installed: Vec<AcpAgent> = nebo_runtimes::detect(&Environment::current())
            .iter()
            .filter_map(|install| install.runtime.acp())
            .collect();
        AcpAgent::KNOWN
            .into_iter()
            .filter(|agent| installed.contains(agent))
            .map(|agent| Hireable {
                key: agent.key(),
                name: agent.name(),
            })
            .collect()
    }

    /// Hosts one more of the coding agent `key` (`claude-code`, `codex`,
    /// ...), in a folder of its own, once it has started and answered in ACP.
    pub async fn hire(&self, key: &str) -> Result<LocalAgent, String> {
        let agent = AcpAgent::KNOWN
            .into_iter()
            .find(|a| a.key() == key)
            .ok_or_else(|| format!("Nebo can't host {key} on this computer."))?;
        let install = nebo_runtimes::detect(&Environment::current())
            .into_iter()
            .find(|install| install.runtime == Runtime::Acp(agent))
            .ok_or_else(|| format!("{} isn't installed on this computer.", agent.name()))?;
        self.host(agent, install.restart).await
    }

    /// Hosts the ACP agent that `command` starts, after starting it once in
    /// its new folder to prove it runs.
    pub(crate) async fn host(
        &self,
        agent: AcpAgent,
        command: RuntimeCommand,
    ) -> Result<LocalAgent, String> {
        if let Some(daemon) = self.hosted_by_daemon() {
            return Err(format!(
                "nebo-link hosts this computer's agents as {daemon}. Hire {} from {daemon}, or unlink it with `nebo-link unlink`.",
                agent.name()
            ));
        }
        let _one = self.hiring.lock().await;
        let (label, id) = {
            let agents = self.agents.lock().expect("local agents");
            let same = agents.iter().filter(|a| a.agent == agent).count();
            let label = match same {
                0 => agent.name().to_owned(),
                n => format!("{} {}", agent.name(), n + 1),
            };
            let taken: Vec<&str> = std::iter::once(link_core::PRIMARY)
                .chain(agents.iter().map(|a| a.id.as_str()))
                .collect();
            let id = link_core::roster::new_id(&label, &taken);
            (label, id)
        };
        let workdir = self.home.join("NeboAI").join(&id);
        std::fs::create_dir_all(&workdir)
            .map_err(|e| format!("Could not make the folder {}: {e}", workdir.display()))?;
        let workdir = workdir.canonicalize().unwrap_or(workdir);
        let title = link_core::acp::probe(agent.name(), &command, &workdir, CLIENT).await?;
        let hosted = LocalAgent {
            label: match (agent, title) {
                (AcpAgent::Other, Some(title)) => title,
                _ => label,
            },
            id,
            agent,
            acp: AcpLink {
                program: command.program,
                args: command.args,
                env: command.env,
                workdir,
            },
        };
        let mut agents = self.agents();
        agents.push(hosted.clone());
        self.save(agents)?;
        info!(agent = %hosted.id, folder = %hosted.acp.workdir.display(), "linked: hosting a coding agent on this computer");
        Ok(hosted)
    }

    /// Stops hosting the agent `id`: its process ends. Its folder stays.
    pub fn remove(&self, id: &str) -> Result<(), String> {
        let mut agents = self.agents();
        let before = agents.len();
        agents.retain(|a| a.id != id);
        if agents.len() == before {
            return Ok(());
        }
        self.save(agents)?;
        info!(
            agent = id,
            "linked: no longer hosting a coding agent on this computer"
        );
        Ok(())
    }

    /// Records `agents` and hosts exactly them: one that stays keeps its
    /// running process and sessions.
    fn save(&self, agents: Vec<LocalAgent>) -> Result<(), String> {
        write_private(&self.dir.join("agents.json"), &agents)
            .map_err(|e| format!("Could not save this computer's agents: {e}"))?;
        let host = self.contract.host();
        let current = host.roster().members();
        let members = agents
            .iter()
            .map(|a| {
                current
                    .iter()
                    .find(|m| m.id == a.id)
                    .cloned()
                    .unwrap_or_else(|| member(&self.dir, a))
            })
            .collect();
        host.set_members(members);
        *self.agents.lock().expect("local agents") = agents;
        Ok(())
    }
}

/// A hosted agent as a member of the roster: its ACP backend, which starts
/// the agent when it is first asked for.
fn member(dir: &Path, agent: &LocalAgent) -> Member {
    let backend = Acp::new(Settings {
        agent: agent.agent,
        name: agent.label.clone(),
        command: agent.acp.command(),
        workdir: agent.acp.workdir.clone(),
        log: dir.join("logs").join(format!("{}.log", agent.id)),
        chats_file: dir.join("agents").join(&agent.id).join("acp-chats.json"),
        client: CLIENT,
    });
    Member {
        id: agent.id.clone(),
        label: agent.label.clone(),
        runtime: agent.agent.key().to_owned(),
        backend: Arc::new(backend),
    }
}

/// Writes `value` as JSON through a temporary file, readable by the owner
/// only.
fn write_private<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(
        serde_json::to_string_pretty(value)
            .expect("agents serialize")
            .as_bytes(),
    )?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)
}
