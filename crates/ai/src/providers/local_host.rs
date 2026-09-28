//! This computer's coding agents, hosted by Nebo itself. Where Nebo runs,
//! Nebo is the host: a Claude Code, Codex, Gemini CLI or OpenCode the owner
//! hires on this computer runs as Nebo's own child process, driven through
//! link-core, with no nebo-link daemon and nothing sent through the hub.
//! Its employee's brain is `linked/<this bot>/<agent>` like any linked
//! agent's; [`super::linked::LinkedProvider`] reaches it here instead of
//! through the hub, over Open Agent Link like any other: this host is an
//! `oal_host::OalHost` Nebo connects to in its own process
//! ([`LocalHost::oal`]), with no network and nothing to encrypt.
//!
//! Nebo is one Open Agent Link device with one key, kept in one key store
//! ([`LocalHost::keys`], `<data dir>/link/oal/`): the key this host is
//! known by, and the key Nebo pairs with every linked bot it reaches.
//!
//! One host per computer per OS user ([`link_core::machine`]): while a
//! nebo-link daemon is linked for this user, it is the host, Nebo hosts
//! nothing, and this computer's agents are hired from the daemon's bot like
//! any other computer's.
//!
//! Every agent works in its own folder, `~/NeboAI/<agent id>`, and a
//! conversation moves to another when the owner asks. Hiring and firing go
//! through link-core's host (`Host::add_agent`, `Host::remove_agent`), the
//! same for every host; what Nebo keeps of them is this record, under
//! `<data dir>/link/`:
//!
//! ```text
//! agents.json                 every hosted agent: its id, name, which agent
//!                             it is, how it starts, its folder
//! agents/<id>/acp-chats.json  the chats an agent that can't list its
//!                             sessions was given
//! logs/<id>.log               each agent's own output
//! oal/                        Nebo's key and every pairing (a KeyStore)
//! oal-seen.json               when each device was last seen
//! ```

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use link_core::acp::{Acp, Client, Settings};
use link_core::host::Host;
use link_core::keep::{Add, Addable, CodingAgent, Installable, Keeper, Kept};
use link_core::roster::{Member, Roster};
use nebo_runtimes::acp::Agent as AcpAgent;
use oal_host::{OalHost, Runtime as OalRuntime};
use oal_secure::KeyStore;
use serde::Serialize;
use tracing::{info, warn};

/// Who drives the agents, as ACP's `initialize` introduces Nebo.
const CLIENT: Client = Client {
    name: "nebo",
    version: env!("CARGO_PKG_VERSION"),
};

/// One coding agent Nebo hosts, as `agents.json` records it.
pub type LocalAgent = CodingAgent;

/// This Nebo's bot id, read per call (a bot gets one when it is first
/// linked to NeboAI). `None` = none yet, so nothing is hosted as it.
pub type BotIdSource = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// This computer's host.
pub struct LocalHost {
    /// This computer's bot: Nebo's own.
    bot_id: BotIdSource,
    /// `<data dir>/link`.
    dir: PathBuf,
    /// Where this OS user's nebo-link daemon would keep its state.
    daemon_home: Option<PathBuf>,
    host: Arc<Host>,
    /// Nebo's key and pairings.
    keys: KeyStore,
    /// The host over Open Agent Link, made once there is a bot to host as.
    oal: OnceLock<Arc<OalHost>>,
    record: Arc<Record>,
}

/// Nebo's record of the agents it hosts (`agents.json`): the host's keeper.
struct Record {
    /// `<data dir>/link`.
    dir: PathBuf,
    /// Where this OS user's nebo-link daemon would keep its state.
    daemon_home: Option<PathBuf>,
    agents: Mutex<Vec<LocalAgent>>,
    /// Agents Nebo was told how to start, besides those installed here.
    told: Mutex<Vec<Installable>>,
}

impl Record {
    fn agents(&self) -> Vec<LocalAgent> {
        self.agents.lock().expect("local agents").clone()
    }

    /// The nebo-link bot hosting this computer's agents, when one is linked.
    fn daemon(&self) -> Option<String> {
        self.daemon_home.as_deref().and_then(link_core::machine::linked_daemon)
    }

    /// Records `agents`, and says where nebo-link looks
    /// ([`link_core::machine::record_app_host`]) whether Nebo hosts this
    /// computer's agents: nebo-link then refuses to link or pair while it
    /// does, so one program hosts them.
    fn save(&self, agents: Vec<LocalAgent>) -> Result<(), String> {
        write_private(&self.dir.join("agents.json"), &agents)
            .map_err(|e| format!("Could not save this computer's agents: {e}"))?;
        self.record_hosting(&agents);
        *self.agents.lock().expect("local agents") = agents;
        Ok(())
    }

    fn record_hosting(&self, agents: &[LocalAgent]) {
        let Some(home) = self.daemon_home.as_deref() else {
            return;
        };
        let hosted: Vec<String> = match self.daemon() {
            Some(_) => Vec::new(),
            None => agents.iter().map(|a| a.label.clone()).collect(),
        };
        if let Err(e) = link_core::machine::record_app_host(home, "Nebo", &hosted) {
            warn!(error = %e, "linked: could not record that Nebo hosts this computer's agents");
        }
    }
}

impl Keeper for Record {
    fn client(&self) -> Client {
        CLIENT
    }

    /// The coding agents installed here, and any Nebo was told how to
    /// start; none while a nebo-link daemon is this computer's host.
    fn addable(&self) -> Vec<Installable> {
        if self.daemon().is_some() {
            return Vec::new();
        }
        let told = self.told.lock().expect("told").clone();
        let mut all = link_core::keep::installed();
        all.retain(|a| !told.iter().any(|t| t.id == a.id));
        all.extend(told);
        all
    }

    fn agents(&self) -> Vec<Kept> {
        Record::agents(self)
            .into_iter()
            .map(|a| Kept {
                runtime: a.agent.key().to_owned(),
                folder: Some(a.acp.workdir.clone()),
                id: a.id,
                label: a.label,
            })
            .collect()
    }

    fn keep(&self, agent: &LocalAgent) -> Result<Member, String> {
        let mut agents = Record::agents(self);
        agents.push(agent.clone());
        self.save(agents)?;
        info!(agent = %agent.id, folder = %agent.acp.workdir.display(), "linked: hosting a coding agent on this computer");
        Ok(member(&self.dir, agent))
    }

    fn forget(&self, id: &str) -> Result<(), String> {
        let mut agents = Record::agents(self);
        agents.retain(|a| a.id != id);
        self.save(agents)?;
        info!(agent = id, "linked: no longer hosting a coding agent on this computer");
        Ok(())
    }
}

impl LocalHost {
    /// The host for Nebo's bot, keeping its record in `dir` and Nebo's key
    /// store in `dir/oal`. `daemon_home` is where a nebo-link daemon of this
    /// OS user keeps its state ([`link_core::machine::daemon_home`]).
    pub fn open(
        bot_id: BotIdSource,
        dir: PathBuf,
        home: PathBuf,
        daemon_home: Option<PathBuf>,
    ) -> Result<Arc<Self>, String> {
        let keys = KeyStore::open(dir.join("oal"))
            .map_err(|e| format!("Could not open Nebo's keys in {}: {e}", dir.join("oal").display()))?;
        let agents: Vec<LocalAgent> = std::fs::read_to_string(dir.join("agents.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let members = agents.iter().map(|a| member(&dir, a)).collect();
        let host = Host::new(Arc::new(Roster::new(members)));
        let record = Arc::new(Record {
            dir: dir.clone(),
            daemon_home: daemon_home.clone(),
            agents: Mutex::new(agents.clone()),
            told: Mutex::new(Vec::new()),
        });
        record.record_hosting(&agents);
        host.set_home(home.clone());
        host.set_keeper(record.clone());
        Ok(Arc::new(Self {
            bot_id,
            dir,
            daemon_home,
            host,
            keys,
            oal: OnceLock::new(),
            record,
        }))
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

    /// Nebo's key and every pairing: the one key store.
    pub fn keys(&self) -> &KeyStore {
        &self.keys
    }

    /// The Open Agent Link host the hosted agents are reached through, in
    /// this process; `None` while a nebo-link daemon is this computer's
    /// host, or before Nebo has a bot to host as.
    pub fn oal(&self) -> Option<Arc<OalHost>> {
        if self.hosted_by_daemon().is_some() {
            return None;
        }
        if let Some(oal) = self.oal.get() {
            return Some(oal.clone());
        }
        let bot_id = self.bot_id()?;
        let record = self.record.clone();
        let config = oal_host::Config {
            host_id: bot_id,
            host_name: "This computer".to_owned(),
            software: ("nebo".to_owned(), env!("CARGO_PKG_VERSION").to_owned()),
            keys: self.keys.clone(),
            seen_file: self.dir.join("oal-seen.json"),
            runtimes: Arc::new(move || {
                let mut all: Vec<OalRuntime> = record
                    .agents()
                    .iter()
                    .map(|a| OalRuntime {
                        id: a.agent.key().to_owned(),
                        name: a.agent.name().to_owned(),
                        kind: "acp".to_owned(),
                        version: None,
                        addable: false,
                    })
                    .collect();
                all.sort_by(|a, b| a.id.cmp(&b.id));
                all
            }),
        };
        match OalHost::new(config, self.host.clone()) {
            Ok(oal) => Some(self.oal.get_or_init(|| oal).clone()),
            Err(e) => {
                warn!(error = %e, "linked: this computer's host could not start");
                None
            }
        }
    }

    /// Every agent Nebo hosts.
    pub fn agents(&self) -> Vec<LocalAgent> {
        self.record.agents()
    }

    /// The coding agents installed on this computer that Nebo can host;
    /// none while a nebo-link daemon is the host.
    pub fn hireable(&self) -> Vec<Addable> {
        self.host.addable()
    }

    /// Hosts one more of the coding agent `key` (`claude-code`, `codex`,
    /// ...), in a folder of its own, once it has started and answered in ACP.
    pub async fn hire(&self, key: &str) -> Result<LocalAgent, String> {
        if let Some(daemon) = self.hosted_by_daemon() {
            let name = AcpAgent::KNOWN.into_iter().find(|a| a.key() == key).map(|a| a.name()).unwrap_or(key);
            return Err(format!(
                "nebo-link hosts this computer's agents as {daemon}. Hire {name} from {daemon}, or unlink it with `nebo-link unlink`."
            ));
        }
        let added = self
            .host
            .add_agent(Add { runtime: key.to_owned(), ..Add::default() })
            .await
            .map_err(|e| e.message)?;
        self.agents()
            .into_iter()
            .find(|a| a.id == added.id)
            .ok_or_else(|| format!("{} was hosted but not recorded.", added.label))
    }

    /// Hosts the ACP agent `agent` as `command` starts it, the way
    /// [`LocalHost::hire`] hosts one installed here: for the tests, whose
    /// agents are scripted (this crate's, and the server's proofs through
    /// the `test-agents` feature).
    #[cfg(any(test, feature = "test-agents"))]
    pub async fn host(&self, agent: AcpAgent, command: nebo_runtimes::RuntimeCommand) -> Result<LocalAgent, String> {
        {
            let mut told = self.record.told.lock().expect("told");
            told.retain(|t| t.id != agent.key());
            told.push(Installable { id: agent.key().to_owned(), name: agent.name().to_owned(), agent, command });
        }
        self.hire(agent.key()).await
    }

    /// Stops hosting the agent `id`: its process ends. Its folder stays.
    pub async fn remove(&self, id: &str) -> Result<(), String> {
        if !self.agents().iter().any(|a| a.id == id) {
            return Ok(());
        }
        self.host.remove_agent(id).await.map(|_| ()).map_err(|e| e.message)
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
