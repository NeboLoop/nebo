//! The linked provider: the brain of an employee hired from a linked bot.
//!
//! A linked bot (a computer's agents joined to the owner's account by
//! `nebo-link`: OpenClaw, Hermes, Claude Code, Codex, ...) serves its agents
//! over Open Agent Link (OAL), and Nebo is an OAL client of it. This
//! provider drives one of its agents in ACP on that agent's channel, the way
//! [`super::cli`] drives a CLI through a process: it reports
//! `handles_tools`, sends the turn, and maps the agent's `session/update`s,
//! its permission requests and the host's `host/turn` into
//! [`StreamEvent`]s, read as the phone reads them ([`link_core::turn`]).
//!
//! The target rides in the model id, `linked/<linkedBotId>/<agentId>`, on the
//! employee's `model_preference`; one provider serves every linked employee.
//! A linked bot is reached directly when it can be (this OS user's
//! nebo-link daemon on this computer, or a host on the LAN), else through a
//! relay ([`Relay`]): NeboAI, at `{NEBOAI_API_URL}/t/{linkedBotId}/oal` with
//! the Nebo bot's own token; every way end-to-end encrypted with the keys
//! Nebo paired with the bot ([`super::oal`]; the first time, Nebo pairs by
//! itself). An agent on this computer that Nebo hosts itself
//! ([`super::local_host::LocalHost`]), under this bot's own id, is reached
//! the same way in memory, with no hub between: one client for both.
//!
//! The agent keeps the transcript. One Nebo chat is one agent session: the
//! first turn on a thread creates the session and records its id and agent
//! on the Nebo chat row (`chats.linked_chat_id`, `chats.linked_agent_id`);
//! every later turn loads it and sends only the newest user message, never
//! the flattened history the CLI provider builds. Nebo's system prompt and
//! steering are not sent — the agent owns its persona. A connection that
//! drops mid-turn is opened again and the session loaded: the turn goes on
//! from where Nebo left it (spec §12).
//!
//! Nebo keeps the conversation; the agent keeps one working stretch of it.
//! A fresh session's first prompt starts with a briefing of who the employee
//! is here ([`ai::LinkedContext`], built by the harness). A healthy session
//! carries on from message to message; the conversation continues in a fresh
//! one, whose first prompt also carries the conversation so far (Nebo's
//! checkpoint summary) and the latest exchanges word for word, when the
//! session stops answering mid-turn ([`Stall`]: its host says the prompt is
//! open and nothing works, held for [`STALL_WINDOW`], never silence alone),
//! when it has used most of its context window as its agent reports, or
//! when it sat idle for [`IDLE_ROTATION`]. The owner reads one plain line
//! when that happens. A cleared conversation starts fresh with no handoff.
//!
//! The owner's `/compact` compacts the agent's own session: it is sent as
//! his message where the agent offers the command (its
//! `available_commands_update`), and refused plainly where it doesn't.
//!
//! A permission request the agent stops for is answered under the
//! employee's permission mode: in Full Access nothing asks, so Nebo answers
//! it with the agent's allow option itself (a runtime with no no-prompt mode
//! of its own still asks). In every other mode it becomes a
//! [`StreamEvent::ask_request`] registered on the run's ask channels, the ONE
//! way a parked question is answered: the app's ask card, the phone (live
//! and on reload), a loop reply, and the owner's spoken answer on his call
//! all show or answer it with the agent's own options, and the option chosen
//! goes back as the request's answer, by the option's kind. The first answer
//! wins: one given elsewhere (the phone) takes Nebo's card back.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
pub use link_core::model::{AgentStatus, Life, SessionStatus, Working};
use link_core::model::{DeviceRef, PermissionOption, StopReason, TurnState, TurnUpdate, code};
use link_core::turn::{self, Permission, ToolEvent, Tools, mode_for};
use nebo_runtimes::acp::protocol::{self, ToolCall as AcpToolCall};
use oal_host::OalHost;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::local_host::LocalHost;
use super::oal::{self, Conn, Unreached};
pub use super::oal::{Relay, TokenSource};
use crate::types::*;

/// The provider id, and the prefix of every linked model id.
pub const ID: &str = "linked";

/// How long a cancel waits for the turn's end.
#[cfg(not(test))]
const CANCEL_TIMEOUT: Duration = Duration::from_secs(10);
/// Short in tests, so a peer that never acknowledges a cancel is exercised
/// in real time (see [`HEARTBEAT`]'s test value for why real time, not a
/// mocked clock).
#[cfg(test)]
const CANCEL_TIMEOUT: Duration = Duration::from_millis(300);
/// How often a connection pings the host, so a turn that waits on the owner
/// keeps it (spec §11: every 20 s of quiet), and checks with `host/agents`
/// that the agent's own process is still there. A linked run can
/// legitimately go silent for a long time — a build, a test suite, a slow
/// tool — with no session/update at all, so liveness is judged by whether
/// the host still reports the process running, never by how long it has
/// been quiet.
#[cfg(not(test))]
const HEARTBEAT: Duration = Duration::from_secs(20);
/// Short in tests, so the liveness check and the "a long silence alone
/// never ends a healthy turn" test both run in real time (this module's
/// tests run a real subprocess over a real socket, which a mocked clock
/// races unpredictably against).
#[cfg(test)]
const HEARTBEAT: Duration = Duration::from_millis(80);
/// How long a turn may sit stalled before its conversation continues in a
/// fresh session ([`Stall`]): its host says the prompt is open and nothing
/// works in it (no call running, no question waiting, none of the agent's
/// processes working) and nothing new has come from the agent. Long enough
/// for a model thinking in silence; a build, a test run or a running call is
/// work the host sees, so it never counts.
#[cfg(not(test))]
const STALL_WINDOW: Duration = Duration::from_secs(10 * 60);
/// Short in tests, in real time (see [`HEARTBEAT`]'s), and longer than any
/// test's own quiet turn.
#[cfg(test)]
const STALL_WINDOW: Duration = Duration::from_secs(3);
/// A session with no turn for this long continues fresh at the next message.
const IDLE_ROTATION: Duration = Duration::from_secs(8 * 3600);
/// The share of its context window (percent), as its agent reports it, a
/// session may use before the conversation continues in a fresh one: ahead
/// of the agent's own compaction, while it still works well.
const LONG_PERCENT: i64 = 60;
/// The most of Nebo's summary a handoff carries.
const SUMMARY_CHARS: usize = 6_000;
/// The latest exchanges a handoff carries word for word, and the most of
/// each message.
const TAIL_MESSAGES: usize = 6;
const TAIL_CHARS: usize = 600;

/// The agent's own command that compacts its session: what the owner's
/// `/compact` runs in a linked employee's conversation.
const COMPACT: &str = "compact";
/// How long Nebo waits, after opening a session, for the commands it
/// offers: an agent says so just after it opens one.
#[cfg(not(test))]
const COMMANDS_WAIT: Duration = Duration::from_secs(5);
#[cfg(test)]
const COMMANDS_WAIT: Duration = Duration::from_millis(300);

/// Waits between tries to reach the linked bot again after a connection
/// drops mid-turn (spec §11: from 1 s, capped at 30 s).
const RECONNECT: [u64; 7] = [1, 2, 4, 8, 16, 30, 30];

/// A [`RECONNECT`] entry's wait: its seconds in production, the same
/// numbers as milliseconds in tests — the same backoff shape, real time
/// either way, just short enough that exhausting every attempt fits in a
/// test.
#[cfg(not(test))]
fn reconnect_wait(secs: u64) -> Duration {
    Duration::from_secs(secs)
}
#[cfg(test)]
fn reconnect_wait(secs: u64) -> Duration {
    Duration::from_millis(secs)
}

/// Nebo as a client of the host on its own computer.
fn this_device() -> DeviceRef {
    DeviceRef {
        device_id: "nebo".to_owned(),
        name: "Nebo".to_owned(),
    }
}

#[derive(Clone)]
pub struct LinkedProvider {
    /// How other computers' linked bots are reached.
    relay: Relay,
    store: Arc<db::Store>,
    /// This computer's host and Nebo's keys.
    local: Option<Arc<LocalHost>>,
    /// What a linked bot calls Nebo once they are paired.
    device_name: String,
}

impl LinkedProvider {
    pub fn new(relay: Relay, store: Arc<db::Store>, local: Option<Arc<LocalHost>>, device_name: &str) -> Self {
        Self {
            relay,
            store,
            local,
            device_name: device_name.to_owned(),
        }
    }

    /// The model id of a linked agent: `linked/<linkedBotId>/<agentId>`.
    pub fn model_id(bot_id: &str, agent_id: &str) -> String {
        format!("{ID}/{bot_id}/{agent_id}")
    }

    /// The linked bot and agent a model id names, when it is a linked one.
    pub fn target(model_id: &str) -> Option<(&str, &str)> {
        model_id
            .strip_prefix(ID)
            .and_then(|rest| rest.strip_prefix('/'))
            .and_then(split)
    }

    /// Pairs Nebo with the linked bot `bot_id` using a pairing code the
    /// owner got on its computer (`nebo-link pair`). Through NeboAI Nebo
    /// pairs by itself; through a self-hosted relay it needs this once.
    pub async fn pair(&self, bot_id: &str, code: &str) -> Result<(), String> {
        let refused = || "That code didn't work. Get a new one on the computer.".to_owned();
        let local = self.local.as_ref().ok_or_else(refused)?;
        let code = oal_secure::PairingCode::parse(code).map_err(|_| refused())?;
        oal::pair(&self.relay, local.direct(), local.keys(), bot_id, &code, &self.device_name)
            .await
            .map(drop)
            .map_err(|_| refused())
    }

    /// Where `bot_id` is reached: this computer's own host when it is this
    /// bot, else through the relay. `Err` is the message the owner reads.
    fn route(&self, bot_id: &str, name: &str) -> Result<Route, String> {
        if let Some(local) = self.local.as_ref().filter(|l| l.bot_id().as_deref() == Some(bot_id)) {
            return match local.oal() {
                Some(oal) => Ok(Route::Local(oal)),
                None => {
                    info!(bot_id, "linked: nebo-link hosts this computer's agents, so Nebo does not");
                    Err(format!("Could not connect to {name}. Try again."))
                }
            };
        }
        match &self.local {
            Some(local) => Ok(Route::Remote(local.clone())),
            None => {
                warn!(bot_id, "linked: Nebo has no key store, so it can't reach another computer");
                Err(format!("Could not connect to {name}. Try again."))
            }
        }
    }

    /// A connection to the host `bot_id` is on.
    async fn open(&self, route: &Route, bot_id: &str) -> Result<Conn, Unreached> {
        match route {
            Route::Local(oal) => Ok(Conn::local(oal, this_device())),
            Route::Remote(local) => oal::connect(&self.relay, local.direct(), local.keys(), bot_id, &self.device_name).await,
        }
    }

    /// One turn, end to end. `Err` is the message the owner reads.
    async fn turn(&self, req: &ChatRequest, bot_id: &str, agent_id: &str, tx: &mpsc::Sender<StreamEvent>) -> Result<(), String> {
        let name = self.employee_name(req, agent_id);
        let route = self.route(bot_id, &name)?;
        let prompt = owners_message(&req.messages).ok_or_else(|| "Nothing to send.".to_owned())?;
        if req.chat_id.is_empty() {
            return Err("This turn belongs to no conversation, so it has no linked chat.".to_owned());
        }
        let chat = self
            .store
            .get_chat(&req.chat_id)
            .map_err(|e| format!("Could not read the conversation: {e}"))?;
        let conn = self.open(&route, bot_id).await.map_err(|e| unreached(e, &name))?;
        let mut driver = Driver {
            provider: self,
            req,
            tx,
            route,
            bot_id: bot_id.to_owned(),
            agent_id: agent_id.to_owned(),
            agent: String::new(),
            folder: None,
            runtime: name.clone(),
            name,
            session: String::new(),
            conn,
            prompt: prompt.to_owned(),
            prompted: false,
            waiting: false,
            turn: None,
            seen: 0,
            tools: Tools::default(),
            asks: HashMap::new(),
            lead: None,
            stall: Stall::default(),
            rotated: false,
            context: None,
        };
        driver.begin(chat).await?;
        let ended = driver.run().await;
        driver.record_activity();
        ended
    }

    /// Whether `bot_id` is this computer's own, hosted by Nebo itself.
    pub fn hosted_here(&self, bot_id: &str) -> bool {
        self.local.as_ref().is_some_and(|l| l.bot_id().as_deref() == Some(bot_id))
    }

    /// What the linked bot `bot_id`'s agents are doing now, as its host
    /// says (`host/status`): for each of `agents` (as the employees' brains
    /// name them), in order, its status, or `None` when the host does not
    /// list it. `Err` is why the host could not be asked, in plain words.
    pub async fn status(&self, bot_id: &str, agents: &[String]) -> Result<Vec<Option<AgentStatus>>, String> {
        let route = self.route(bot_id, COMPUTER)?;
        let mut conn = self.open(&route, bot_id).await.map_err(|e| unreached(e, COMPUTER))?;
        let (answer, _) = conn.call(None, "host/status", json!({})).await.ok_or_else(no_answer)?;
        let answer = answer.map_err(|e| e.message)?;
        let listed: Vec<AgentStatus> =
            serde_json::from_value(answer["agents"].clone()).map_err(|e| format!("its computer's status could not be read: {e}"))?;
        Ok(agents
            .iter()
            .map(|agent| {
                let canonical = link_core::roster::agent_id(agent);
                listed.iter().find(|s| s.agent == *agent || s.agent == canonical).cloned()
            })
            .collect())
    }

    /// What the agent `agent_id` of `bot_id` is doing in its session
    /// `session_id`, read from the session's record on its host
    /// (`session/load`, which reads and changes nothing): see
    /// [`SessionWork`]. Only for a session its host holds open: loading a
    /// paused one would start the agent again.
    pub async fn session_work(&self, bot_id: &str, agent_id: &str, session_id: &str) -> Result<SessionWork, String> {
        let route = self.route(bot_id, COMPUTER)?;
        let mut conn = self.open(&route, bot_id).await.map_err(|e| unreached(e, COMPUTER))?;
        let (answer, _) = conn.call(None, "host/agents", json!({})).await.ok_or_else(no_answer)?;
        let agents = answer.map_err(|e| e.message)?;
        let listed = agents["agents"].as_array().cloned().unwrap_or_default();
        let found = listed_agent(&listed, agent_id).ok_or_else(|| format!("No agent {agent_id} on its computer."))?;
        let agent = found["id"].as_str().unwrap_or_default().to_owned();
        let cwd = found["folder"].as_str().unwrap_or("/").to_owned();
        let params = json!({ "protocolVersion": 1, "clientCapabilities": {} });
        let (answer, _) = conn.call(Some(&agent), "initialize", params).await.ok_or_else(no_answer)?;
        answer.map_err(|e| e.message)?;
        let params = json!({ "sessionId": session_id, "cwd": cwd, "mcpServers": [] });
        let (answer, replay) = conn.call(Some(&agent), "session/load", params).await.ok_or_else(no_answer)?;
        answer.map_err(|e| e.message)?;
        let mut record = Vec::new();
        let mut turn = None;
        for frame in &replay {
            match replayed(frame, &agent, session_id) {
                Replayed::Turn(update) => turn = Some(update),
                Replayed::Update(update) => record.push(update),
                Replayed::Other => {}
            }
        }
        Ok(summarise(&record, turn.filter(|t| t.state == TurnState::Running).as_ref()))
    }

    /// The employee's name, for the copy the owner reads.
    fn employee_name(&self, req: &ChatRequest, agent_id: &str) -> String {
        self.store
            .get_agent(&req.trace.agent_id)
            .ok()
            .flatten()
            .map(|a| a.name)
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| agent_id.to_owned())
    }
}

#[async_trait]
impl Provider for LinkedProvider {
    fn id(&self) -> &str {
        ID
    }

    fn handles_tools(&self) -> bool {
        true
    }

    /// The agent keeps the transcript: a message is delivered once, and the
    /// provider answers only for the agent it is addressed to.
    fn retryable(&self) -> bool {
        false
    }

    /// A stop here is an ACP `session/cancel` to the linked agent, waited on
    /// ([`Driver::run`]'s own cancel branch): the runner must not treat this
    /// call as over, and free to send another, before that round trip ends.
    fn cancel_is_async(&self) -> bool {
        true
    }

    fn linked(&self) -> Option<&LinkedProvider> {
        Some(self)
    }

    async fn stream(&self, req: &ChatRequest) -> Result<EventReceiver, ProviderError> {
        // The runner hands the provider its model without the `linked/`
        // prefix, as it does every provider.
        let Some((bot_id, agent_id)) = split(&req.model) else {
            return Err(ProviderError::Request(format!("not a linked agent: {:?}", req.model)));
        };
        let (bot_id, agent_id) = (bot_id.to_owned(), agent_id.to_owned());
        let (tx, rx) = mpsc::channel(100);
        let this = self.clone();
        let req = req.clone();
        tokio::spawn(async move {
            if let Err(message) = this.turn(&req, &bot_id, &agent_id, &tx).await {
                let _ = tx.send(StreamEvent::error(message)).await;
                let _ = tx.send(StreamEvent::done()).await;
            }
        });
        Ok(rx)
    }
}

/// Where a linked agent is reached.
enum Route {
    /// This computer's own host, in this process.
    Local(Arc<OalHost>),
    /// A linked bot's host (another computer's, or this OS user's nebo-link
    /// daemon's), directly when it can be, else through the relay, with
    /// Nebo's keys.
    Remote(Arc<LocalHost>),
}

/// [`Driver::liveness`]'s answer.
enum Liveness {
    /// The host says the agent's own process is no longer running.
    Gone,
    /// The turn stopped answering ([`Stall`]).
    Stalled,
    /// No definite answer — a blip, or nothing worth acting on.
    Unknown,
    /// A frame read while checking turned out to be this turn's own end.
    Ended(TurnUpdate),
}

/// A permission request Nebo raised a card for.
struct Asked {
    /// The id of this connection's copy of the request (`None` after a
    /// reconnect, until the host sends the request again).
    rpc: Option<Value>,
    options: Vec<PermissionOption>,
    labels: Vec<String>,
    /// The option the owner chose, sent again to a copy the host sends after
    /// a reconnect until the host says the request is resolved.
    chosen: Option<String>,
}

/// One turn on one connection (opened again if it drops).
struct Driver<'a> {
    provider: &'a LinkedProvider,
    req: &'a ChatRequest,
    tx: &'a mpsc::Sender<StreamEvent>,
    route: Route,
    bot_id: String,
    /// The agent as the employee's brain names it.
    agent_id: String,
    /// The agent's id on the host: its channel.
    agent: String,
    /// The folder its sessions work in, when it has one.
    folder: Option<String>,
    /// The employee's name, for the copy the owner reads.
    name: String,
    /// What the agent calls itself on its host ("Claude Code").
    runtime: String,
    session: String,
    conn: Conn,
    prompt: String,
    /// The prompt was sent and not refused.
    prompted: bool,
    /// The prompt was refused while another turn runs in the session: it is
    /// sent again when that one ends.
    waiting: bool,
    /// This turn's id, once the host started it.
    turn: Option<String>,
    /// How many of this turn's updates were read.
    seen: usize,
    tools: Tools,
    /// The permission requests asked, by their id on the run's ask channels
    /// (the tool call's id).
    asks: HashMap<String, Asked>,
    /// The first block of a fresh session's first prompt: its briefing and,
    /// when it carries the conversation on, the handoff.
    lead: Option<String>,
    /// Whether this turn is stalled, one heartbeat at a time.
    stall: Stall,
    /// The conversation already continued in a fresh session this turn.
    rotated: bool,
    /// How much of its context window the session has used, and the
    /// window, as the agent last reported this turn (`usage_update`).
    context: Option<(i64, i64)>,
}

impl Driver<'_> {
    fn offline(&self) -> String {
        format!("Could not connect to {}. Try again.", self.name)
    }

    /// The agent, its session (made on the chat's first turn, loaded on every
    /// other), the employee's mode, and the prompt sent.
    async fn begin(&mut self, chat: Option<db::models::Chat>) -> Result<(), String> {
        self.locate().await?;
        let capabilities = self.initialize().await?;
        // The owner's `/compact` works on the session as it is: it never
        // rotates it.
        let compact = is_command(&self.prompt, COMPACT);
        let due = chat.as_ref().filter(|_| !compact).and_then(|c| rotation_due(c, now_secs()));
        let recorded = chat.and_then(|c| {
            let session = c.linked_chat_id.filter(|id| !id.is_empty())?;
            // A session recorded before its agent was is the employee's.
            let unrecorded = c.linked_agent_id.is_none();
            let agent = c.linked_agent_id.unwrap_or_else(|| self.agent_id.clone());
            (agent == self.agent_id).then_some((session, unrecorded))
        });
        let opened = match (recorded, due) {
            // A session that got long or sat idle: the conversation continues
            // in a fresh one, with the handoff.
            (Some(_), Some(why)) => return self.rotate(why).await,
            (Some((session, unrecorded)), None) => {
                // `/compact` loads the session: its record says what
                // commands it offers.
                let resume = capabilities["sessionCapabilities"]["resume"].is_object() && !compact;
                let method = if resume { "session/resume" } else { "session/load" };
                let params = json!({ "sessionId": session, "cwd": self.cwd(), "mcpServers": [] });
                let agent = self.agent.clone();
                let (answer, replay) = self.conn.call(Some(&agent), method, params).await.ok_or_else(|| self.offline())?;
                let opened = answer.map_err(|e| turn::plain(&self.name, &e))?;
                self.session = session;
                if compact && !self.offers(COMPACT, &replay).await {
                    info!(session = %self.session, "linked: the agent offers no compact command");
                    return Err(format!("{} can't compact its conversation from here.", self.runtime));
                }
                if unrecorded {
                    self.record()?;
                }
                // Where it works now: its agent's folder, or where the owner
                // asked it to move.
                if let Some(folder) = opened["_meta"][link_core::host::META_CWD].as_str().or(self.folder.as_deref()) {
                    self.record_folder(folder);
                }
                opened
            }
            (None, _) if compact => return Err(format!("{} has nothing to compact yet.", self.runtime)),
            // The chat's first turn, or a cleared conversation: a fresh
            // session with its briefing and no handoff.
            (None, _) => {
                let lead = self.lead(false).await;
                self.new_session(lead).await?
            }
        };
        self.mode(&opened).await?;
        self.send_prompt()
    }

    /// A fresh session of the agent for this conversation, recorded on the
    /// chat, its first prompt to start with `lead`.
    async fn new_session(&mut self, lead: Option<String>) -> Result<Value, String> {
        let params = json!({ "cwd": self.cwd(), "mcpServers": [] });
        let agent = self.agent.clone();
        let (answer, _) = self.conn.call(Some(&agent), "session/new", params).await.ok_or_else(|| self.offline())?;
        let created = answer.map_err(|e| turn::plain(&self.name, &e))?;
        self.session = created["sessionId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "The linked bot created a chat without an id.".to_owned())?
            .to_owned();
        self.record()?;
        if let Some(folder) = self.folder.clone() {
            self.record_folder(&folder);
        }
        self.lead = lead;
        info!(bot_id = %self.bot_id, agent = %self.agent, chat_id = %self.req.chat_id, session = %self.session, "linked: session created");
        Ok(created)
    }

    /// The conversation continues in a fresh session of the agent (`why`):
    /// the old one is forgotten, the owner reads one plain line, and the
    /// fresh session's first prompt carries the handoff and the owner's
    /// message. Once a turn.
    async fn rotate(&mut self, why: Rotation) -> Result<(), String> {
        info!(chat_id = %self.req.chat_id, session = %self.session, ?why, "linked: the conversation continues in a fresh session");
        self.forget_session();
        self.rotated = true;
        let lead = self.lead(true).await;
        let notice = format!("Continuing in a fresh {} session with a summary of this conversation.\n\n", self.runtime);
        let _ = self.tx.send(StreamEvent::text(notice)).await;
        self.turn = None;
        self.seen = 0;
        self.tools = Tools::default();
        self.asks.clear();
        self.prompted = false;
        self.waiting = false;
        self.stall = Stall::default();
        self.context = None;
        let opened = self.new_session(lead).await?;
        self.mode(&opened).await?;
        self.send_prompt()
    }

    /// The first block of a fresh session's first prompt: the briefing and,
    /// when it carries the conversation on, the conversation so far and the
    /// latest exchanges word for word. `None` with nothing to say.
    async fn lead(&mut self, carrying_on: bool) -> Option<String> {
        let context = self.req.linked_context.as_ref().map(|c| c.0.clone());
        let mut parts: Vec<String> = Vec::new();
        if let Some(briefing) = context.as_ref().map(|c| c.briefing()).filter(|b| !b.trim().is_empty()) {
            parts.push(briefing);
        }
        if carrying_on {
            parts.push("This conversation carries on from an earlier session. Pick up from where it stands; don't ask the owner to repeat anything.".to_owned());
            let summary = match &context {
                Some(context) => context.summary().await,
                None => None,
            };
            if let Some(summary) = summary.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                parts.push(format!("## The conversation so far\n\n{}", clip(summary, SUMMARY_CHARS, false)));
            }
            if let Some(tail) = latest_exchanges(&self.req.messages) {
                parts.push(format!("## The latest exchanges, word for word\n\n{tail}"));
            }
        }
        (!parts.is_empty()).then(|| parts.join("\n\n"))
    }

    /// Whether the session offers the agent's own command `name`, as its
    /// `available_commands_update` says: read from what opening it replayed,
    /// or waited for ([`COMMANDS_WAIT`]), since an agent says it just after
    /// it opens a session.
    async fn offers(&mut self, name: &str, replay: &[Value]) -> bool {
        let mut offered = replay.iter().filter_map(|frame| self.commands_in(frame)).last();
        let deadline = tokio::time::Instant::now() + COMMANDS_WAIT;
        while offered.is_none() {
            match tokio::time::timeout_at(deadline, self.conn.next()).await {
                Ok(Some(frame)) => offered = self.commands_in(&frame),
                _ => break,
            }
        }
        offered.is_some_and(|names| names.iter().any(|n| n.eq_ignore_ascii_case(name)))
    }

    /// The commands a frame says this session offers, when it says so.
    fn commands_in(&self, frame: &Value) -> Option<Vec<String>> {
        let params = &frame["acp"]["params"];
        let said = frame["agent"] == self.agent.as_str()
            && frame["acp"]["method"] == "session/update"
            && params["sessionId"] == self.session.as_str()
            && params["update"]["sessionUpdate"] == "available_commands_update";
        said.then(|| {
            params["update"]["availableCommands"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| c["name"].as_str().map(|n| n.trim_start_matches('/').to_owned()))
                .collect()
        })
    }

    /// Records this turn on the chat: when it was, and how much of its
    /// context window the session has used, as the agent reported.
    fn record_activity(&self) {
        if self.session.is_empty() {
            return;
        }
        if let Err(e) = self.provider.store.set_chat_linked_activity(&self.req.chat_id, now_secs(), self.context) {
            warn!(chat_id = %self.req.chat_id, error = %e, "linked: the session's activity was not recorded");
        }
    }

    /// The agent on the host (`host/agents`): by the id the brain names, or
    /// by the one form every agent id now takes.
    async fn locate(&mut self) -> Result<(), String> {
        let (answer, _) = self.conn.call(None, "host/agents", json!({})).await.ok_or_else(|| self.offline())?;
        let agents = answer.map_err(|e| turn::plain(&self.name, &e))?;
        let listed = agents["agents"].as_array().cloned().unwrap_or_default();
        let found = listed_agent(&listed, &self.agent_id).ok_or_else(|| format!("No agent {} on this bot.", self.agent_id))?;
        self.agent = found["id"].as_str().unwrap_or_default().to_owned();
        self.folder = found["folder"].as_str().map(str::to_owned);
        let runtime = found["runtime"].as_str().unwrap_or_default();
        let known = nebo_runtimes::acp::Agent::KNOWN.iter().find(|a| a.key() == runtime).map(|a| a.name().to_owned());
        if let Some(runtime) = known.or_else(|| found["label"].as_str().filter(|l| !l.trim().is_empty()).map(str::to_owned)) {
            self.runtime = runtime;
        }
        Ok(())
    }

    /// `initialize` on the agent's channel: its capabilities.
    async fn initialize(&mut self) -> Result<Value, String> {
        let params = json!({ "protocolVersion": 1, "clientCapabilities": {} });
        let agent = self.agent.clone();
        let (answer, _) = self.conn.call(Some(&agent), "initialize", params).await.ok_or_else(|| self.offline())?;
        Ok(answer.map_err(|e| turn::plain(&self.name, &e))?["agentCapabilities"].clone())
    }

    fn cwd(&self) -> String {
        self.folder.clone().unwrap_or_else(|| "/".to_owned())
    }

    /// Records the session and its agent on the chat row.
    fn record(&self) -> Result<(), String> {
        self.provider
            .store
            .set_chat_linked_session(&self.req.chat_id, &self.agent_id, &self.session)
            .map_err(|e| format!("Could not record the linked chat: {e}"))
    }

    /// Records the folder the conversation works in on the chat row, which
    /// the app shows ("Works in ~/workspaces/foo").
    fn record_folder(&self, folder: &str) {
        if let Err(e) = self.provider.store.set_chat_linked_folder(&self.req.chat_id, folder) {
            warn!(chat_id = %self.req.chat_id, error = %e, "linked: the conversation's folder was not recorded");
        }
    }

    /// The employee's permission mode rides with the turn: an agent with
    /// modes runs it in its own matching one, set before every message.
    /// The mode the session says it is in is not trusted to skip it: a
    /// host answers from its own record of the session, and an agent
    /// process started again comes back in whatever mode its own settings
    /// start it in.
    async fn mode(&mut self, opened: &Value) -> Result<(), String> {
        let Some(permission) = self.req.permission_mode.and_then(|m| Permission::parse(m.as_str())) else {
            return Ok(());
        };
        let Some(modes) = protocol::modes(opened) else {
            return Ok(());
        };
        let Some(wanted) = mode_for(permission, &modes.available) else {
            return Ok(());
        };
        let wanted = wanted.to_owned();
        let params = json!({ "sessionId": self.session, "modeId": wanted });
        let agent = self.agent.clone();
        let (answer, _) = self.conn.call(Some(&agent), "session/set_mode", params).await.ok_or_else(|| self.offline())?;
        answer.map_err(|e| format!("{} could not switch to its {wanted} mode: {}", self.name, e.message))?;
        info!(session = %self.session, mode = %wanted, was = %modes.current, ?permission, "linked: session mode set");
        Ok(())
    }

    fn send_prompt(&mut self) -> Result<(), String> {
        let mut prompt: Vec<Value> = Vec::new();
        if let Some(lead) = &self.lead {
            prompt.push(json!({ "type": "text", "text": lead }));
        }
        prompt.push(json!({ "type": "text", "text": self.prompt }));
        let params = json!({ "sessionId": self.session, "prompt": prompt });
        let agent = self.agent.clone();
        if self.conn.request(Some(&agent), "session/prompt", params).is_none() {
            return Err(self.offline());
        }
        self.prompted = true;
        self.waiting = false;
        info!(bot_id = %self.bot_id, agent = %self.agent, session = %self.session, prompt_len = self.prompt.len(), "linked: turn sent");
        Ok(())
    }

    /// What the owner reads when Nebo gives up on the agent's own process,
    /// found gone on the host (`host/agents`' `online`): plain, no
    /// wake/sleep/restart words, just what to do.
    fn stalled_notice(&self) -> String {
        format!("{} didn't answer. Try again.", self.name)
    }

    /// A cancel that never got an answer — Nebo's own [`CANCEL_TIMEOUT`]
    /// ran out, or the connection closed while one was pending — is the one
    /// case nothing tells Nebo the agent's own side is sane: the session
    /// may be wedged, not merely idle. Forgetting it here means the next
    /// message on this chat opens a fresh one instead of resuming one that
    /// might never answer again. An *acknowledged* cancel (the agent said
    /// "cancelled", or the host says the process died but its own record of
    /// the session survives it) keeps the session — the owner's "Stop… now
    /// do X" still has its context. A session the conversation leaves for a
    /// fresh one ([`Driver::rotate`]) is forgotten too.
    fn forget_session(&self) {
        if let Err(e) = self.provider.store.set_chat_linked_session(&self.req.chat_id, &self.agent_id, "") {
            warn!(chat_id = %self.req.chat_id, error = %e, "linked: could not forget the session");
        }
    }

    /// Whether the host still lists this turn's agent as running its
    /// process (`host/agents`' `online`), without losing anything else the
    /// connection delivers while waiting for that answer (`conn.call`'s
    /// `others`, read verbatim through [`Driver::frame`] the way the main
    /// loop reads every frame — this call runs mid-turn, not before the
    /// prompt like `locate`'s, so something real can arrive alongside it).
    /// A blip — no answer, no listing, no `online` field — proves nothing
    /// either way: liveness still rests on the connection and the reconnect
    /// window then, never on one missed status call.
    ///
    /// Then what the host says the agent is doing (`host/status`), taken in
    /// by [`Stall`]: the turn is stalled once its prompt has been open with
    /// nothing working and nothing new for [`STALL_WINDOW`]. A host that
    /// doesn't say, or a turn not yet started, is never judged.
    async fn liveness(&mut self, answers: &mpsc::Sender<(String, String)>) -> Result<Liveness, String> {
        let Some((answer, others)) = self.conn.call(None, "host/agents", json!({})).await else {
            return Ok(Liveness::Unknown);
        };
        for other in others {
            if let Some(ended) = self.frame(other, answers).await? {
                return Ok(Liveness::Ended(ended));
            }
        }
        let Ok(agents) = answer else {
            return Ok(Liveness::Unknown);
        };
        let listed = agents["agents"].as_array().cloned().unwrap_or_default();
        if listed.iter().find(|a| a["id"] == self.agent.as_str()).and_then(|a| a["online"].as_bool()) == Some(false) {
            return Ok(Liveness::Gone);
        }
        let Some((answer, others)) = self.conn.call(None, "host/status", json!({})).await else {
            self.stall.look(None, &self.session, std::time::Instant::now(), STALL_WINDOW);
            return Ok(Liveness::Unknown);
        };
        for other in others {
            if let Some(ended) = self.frame(other, answers).await? {
                return Ok(Liveness::Ended(ended));
            }
        }
        let status = answer
            .ok()
            .and_then(|a| serde_json::from_value::<Vec<AgentStatus>>(a["agents"].clone()).ok())
            .and_then(|listed| listed.into_iter().find(|s| s.agent == self.agent));
        let status = status.filter(|_| self.turn.is_some());
        match self.stall.look(status.as_ref(), &self.session, std::time::Instant::now(), STALL_WINDOW) {
            true => Ok(Liveness::Stalled),
            false => Ok(Liveness::Unknown),
        }
    }

    /// The turn, from the prompt to its end.
    async fn run(&mut self) -> Result<(), String> {
        let (answers_tx, mut answers_rx) = mpsc::channel::<(String, String)>(8);
        let cancel = self.req.cancel_token.clone().unwrap_or_default();
        let mut cancel_deadline: Option<tokio::time::Instant> = None;
        let mut heartbeat = tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT, HEARTBEAT);
        loop {
            tokio::select! {
                _ = cancel.cancelled(), if cancel_deadline.is_none() => {
                    // A message still waiting behind another turn in the
                    // session (its prompt refused as turn_in_progress) was
                    // never taken: stopping it withdraws it. The running
                    // turn is not this one's to cancel.
                    if self.waiting {
                        info!(session = %self.session, "linked: a waiting message was withdrawn; the running turn goes on");
                        return Err("Cancelled".to_owned());
                    }
                    let cancel = json!({ "agent": self.agent, "acp": { "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": self.session } } });
                    if !self.conn.send(&cancel) {
                        return Err("Cancelled".to_owned());
                    }
                    cancel_deadline = Some(tokio::time::Instant::now() + CANCEL_TIMEOUT);
                }
                // The cancel never got an answer: forget the session, the
                // one case nothing vouches for its state.
                _ = tokio::time::sleep_until(cancel_deadline.unwrap_or_else(tokio::time::Instant::now)), if cancel_deadline.is_some() => {
                    self.forget_session();
                    return Err("Cancelled".to_owned());
                }
                Some((id, value)) = answers_rx.recv() => self.answer(&id, &value),
                // A run can legitimately go silent for a long time — a
                // build, a test suite, a slow tool — with no session/update
                // at all, so this never judges liveness by quiet alone.
                // Every heartbeat it also asks the host whether the
                // agent's own process is still there
                // (`host/agents`'s `online`): a connection can stay open to
                // a host whose agent process already died.
                _ = heartbeat.tick() => {
                    let _ = self.conn.request(None, "host/ping", json!({}));
                    if cancel_deadline.is_none() {
                        match self.liveness(&answers_tx).await? {
                            Liveness::Ended(ended) => return self.ended(ended).await,
                            Liveness::Gone => {
                                // The process died, but the session itself
                                // (the agent's own transcript) is not lost:
                                // a fresh adapter resumes it via
                                // session/load, so the session is kept.
                                warn!(bot_id = %self.bot_id, agent = %self.agent, session = %self.session, "linked: the host says the agent's process is gone; ending the turn");
                                let cancel = json!({ "agent": self.agent, "acp": { "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": self.session } } });
                                let _ = self.conn.send(&cancel);
                                return Err(self.stalled_notice());
                            }
                            Liveness::Stalled => {
                                warn!(bot_id = %self.bot_id, agent = %self.agent, session = %self.session, "linked: the turn stopped answering: its prompt is open and nothing works in it");
                                let cancel = json!({ "agent": self.agent, "acp": { "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": self.session } } });
                                let _ = self.conn.send(&cancel);
                                if self.rotated {
                                    self.forget_session();
                                    return Err(self.stalled_notice());
                                }
                                self.rotate(Rotation::Stalled).await?;
                            }
                            Liveness::Unknown => {}
                        }
                    }
                }
                frame = self.conn.next() => {
                    let ended = match frame {
                        Some(frame) => self.frame(frame, &answers_tx).await?,
                        None if cancel_deadline.is_some() => {
                            self.forget_session();
                            return Err("Cancelled".to_owned());
                        }
                        None => self.reconnect(&answers_tx, &cancel).await?,
                    };
                    if let Some(ended) = ended {
                        return self.ended(ended).await;
                    }
                }
            }
        }
    }

    /// One frame from the host; the turn's end when it is that.
    async fn frame(&mut self, frame: Value, answers: &mpsc::Sender<(String, String)>) -> Result<Option<TurnUpdate>, String> {
        if frame.get("agent").is_none() {
            if frame["method"] == "host/turn" {
                return self.host_turn(&frame["params"]);
            }
            return Ok(None);
        }
        if frame["agent"] != self.agent.as_str() {
            return Ok(None);
        }
        let msg = &frame["acp"];
        let ours = msg["params"]["sessionId"] == self.session.as_str();
        match (msg["method"].as_str(), msg.get("id")) {
            (Some("session/update"), None) if ours => {
                self.stall.heard();
                if self.turn.is_some() {
                    self.seen += 1;
                    self.update(&msg["params"]["update"]).await;
                }
            }
            (Some("session/request_permission"), Some(id)) if ours => {
                self.stall.heard();
                self.asked(id.clone(), &msg["params"], answers).await;
            }
            (Some("$/cancel_request"), None) => self.withdrawn(&msg["params"]["requestId"]).await,
            (None, Some(_)) => {
                // The prompt's answer: an error before the turn started is
                // a refusal (or a turn already running, which this one waits
                // behind); anything else the turn's end says.
                if let Err(e) = oal::answer(msg)
                    && self.turn.is_none()
                    && self.prompted
                {
                    if e.code != code::TURN_IN_PROGRESS {
                        return Err(turn::plain(&self.name, &e));
                    }
                    self.prompted = false;
                    self.waiting = true;
                }
            }
            _ => {}
        }
        Ok(None)
    }

    /// `host/turn` for the session: this turn starting or ending, or the one
    /// this turn waits behind ending.
    fn host_turn(&mut self, params: &Value) -> Result<Option<TurnUpdate>, String> {
        let Ok(update) = serde_json::from_value::<TurnUpdate>(params.clone()) else {
            return Ok(None);
        };
        if update.agent != self.agent || update.session_id != self.session {
            return Ok(None);
        }
        let ours = update.by.as_ref().is_some_and(|d| d.device_id == self.conn.device);
        match update.state {
            TurnState::Running if self.turn.is_none() && self.prompted && ours => self.turn = Some(update.turn_id),
            TurnState::Ended if self.turn.as_deref() == Some(update.turn_id.as_str()) => return Ok(Some(update)),
            TurnState::Ended if self.waiting => self.send_prompt()?,
            _ => {}
        }
        Ok(None)
    }

    /// One of this turn's updates, as stream events.
    async fn update(&mut self, update: &Value) {
        // How much of its context window the session has used, as the agent
        // reports it: what says the conversation should continue fresh.
        if update["sessionUpdate"] == "usage_update"
            && let (Some(used), Some(size)) = (update["used"].as_i64(), update["size"].as_i64())
        {
            self.context = Some((used, size));
        }
        // The conversation moved to another folder at the owner's request:
        // the host says where ("Now working in …" arrives as its text).
        if update["sessionUpdate"] == "session_info_update"
            && let Some(folder) = update["_meta"][link_core::host::META_CWD].as_str()
        {
            info!(chat_id = %self.req.chat_id, folder, "linked: the conversation moved to another folder");
            self.record_folder(folder);
        }
        let Some((_, parsed)) = protocol::update(&json!({ "sessionId": self.session, "update": update })) else {
            return;
        };
        match parsed {
            protocol::Update::AgentText { text, .. } => {
                self.tools.flush();
                self.cards().await;
                if !text.is_empty() {
                    let _ = self.tx.send(StreamEvent::text(text)).await;
                }
            }
            protocol::Update::Thought(text) => {
                if !text.is_empty() {
                    let _ = self.tx.send(StreamEvent::thinking(text)).await;
                }
            }
            protocol::Update::Plan(entries) => {
                let _ = self.tx.send(StreamEvent::thinking(turn::plan(&entries))).await;
            }
            protocol::Update::ToolCall(call) | protocol::Update::ToolCallUpdate(call) => {
                self.tools.tool(call, update["_meta"]["durationMs"].as_u64());
                self.cards().await;
            }
            _ => {}
        }
    }

    /// The tool cards made since last sent.
    async fn cards(&mut self) {
        for card in self.tools.take() {
            let event = match card {
                ToolEvent::Started { id, name, input } => StreamEvent::tool_call(ToolCall { id, name, input }),
                ToolEvent::Finished { id, name, output, failed, .. } => StreamEvent {
                    payload: None,
                    provenance: None,
                    event_type: StreamEventType::ToolResult,
                    text: output,
                    tool_call: Some(ToolCall { id, name, input: Value::Null }),
                    error: failed.then(|| "tool error".to_owned()),
                    usage: None,
                    rate_limit: None,
                    widgets: None,
                    provider_metadata: None,
                    stop_reason: None,
                    image_url: None,
                },
            };
            let _ = self.tx.send(event).await;
        }
    }

    /// The agent stopped to ask the owner. In Full Access it is allowed
    /// here; otherwise the question is registered on the run's ask channels
    /// and raised as `ask_request` with the agent's own options, and the
    /// option chosen comes back through `answers`. A request sent again
    /// after a reconnect is the same question.
    async fn asked(&mut self, rpc: Value, params: &Value, answers: &mpsc::Sender<(String, String)>) {
        let call = AcpToolCall::parse(&params["toolCall"]).unwrap_or_default();
        let id = match call.id.as_str() {
            "" => format!("ask-{rpc}"),
            id => id.to_owned(),
        };
        if let Some(asked) = self.asks.get_mut(&id) {
            asked.rpc = Some(rpc);
            self.send_answer(&id);
            return;
        }
        let options: Vec<PermissionOption> = serde_json::from_value(params["options"].clone()).unwrap_or_default();
        let words = match turn::words_in(params) {
            Some(words) => words,
            None => {
                let words = self.tools.ask(call, &options);
                self.cards().await;
                words
            }
        };
        let owners_answers: Vec<Option<&str>> = options.iter().map(|o| answer_of(&o.kind)).collect();
        // What Full Access answers with: the agent's allow, once if it
        // offers that.
        let allow = ["allow_once", "allow_always"]
            .iter()
            .find_map(|kind| options.iter().find(|o| o.kind == *kind))
            .map(|o| o.option_id.clone());
        self.asks.insert(
            id.clone(),
            Asked {
                rpc: Some(rpc),
                options,
                labels: words.labels.clone(),
                chosen: None,
            },
        );
        // Full Access: nothing asks. The agent runs in its own no-prompt
        // mode when it has one; one that still asks is answered here.
        if self.req.permission_mode == Some(types::permissions::Mode::FullAccess)
            && let Some(allow) = allow
        {
            info!(request_id = %id, "linked: full access, so Nebo allows the agent's ask itself");
            self.answer(&id, &allow);
            return;
        }
        // A question nobody here can answer is never left waiting: the call
        // is refused, as Nebo refuses its own calls when nothing can wait
        // for the owner.
        if owners_answers.iter().all(Option::is_none) {
            warn!(request_id = %id, "linked: the agent asked with no option the owner can pick; it is cancelled");
            self.cancel_ask(&id);
            return;
        }
        let Some(channels) = self.req.ask_channels.as_ref() else {
            warn!(request_id = %id, "linked: the agent asked on a run nothing can answer; it is declined");
            self.answer(&id, "no");
            if self.asks.get(&id).is_some_and(|a| a.chosen.is_none()) {
                self.cancel_ask(&id);
            }
            return;
        };
        info!(request_id = %id, "linked: the agent asks the owner");
        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        channels.lock().await.insert(id.clone(), resp_tx);
        // Every option the agent offered, and the owner's answer each one
        // is (`answers`, by the option's kind): what a spoken answer on his
        // call is matched to.
        let widgets = json!([{ "type": "options", "multiSelect": false, "options": words.labels, "answers": owners_answers }]);
        let _ = self.tx.send(StreamEvent::ask_request(id.clone(), words.question, Some(widgets))).await;
        let answers = answers.clone();
        tokio::spawn(async move {
            // A question nobody answers (the run ended) is not answered: the
            // host resolves it with its turn.
            if let Ok(value) = resp_rx.await {
                let _ = answers.send((id, value)).await;
            }
        });
    }

    /// The owner's answer to the question `id`, sent as the option it
    /// names: the owner's answer (`this_once`, `allow_always`, `no`) as the
    /// option of its kind, or the card's label for an option, or the
    /// option's id. The first answer is the answer; another is ignored.
    fn answer(&mut self, id: &str, value: &str) {
        let Some(asked) = self.asks.get_mut(id) else {
            info!(request_id = id, "linked: an answer for a question nobody is waiting on");
            return;
        };
        if asked.chosen.is_some() {
            info!(request_id = id, value, "linked: the question was already answered");
            return;
        }
        let by_kind = asked.options.iter().find(|o| answer_of(&o.kind) == Some(value));
        let by_label = || {
            asked
                .options
                .iter()
                .zip(asked.labels.iter().map(Some).chain(std::iter::repeat(None)))
                .find(|(o, label)| label.is_some_and(|l| l == value) || o.option_id == value)
                .map(|(o, _)| o)
        };
        let Some(option) = by_kind.or_else(by_label) else {
            info!(request_id = id, value, "linked: an answer that is none of the question's options");
            return;
        };
        info!(request_id = id, option = %option.option_id, kind = %option.kind, "linked: the answer goes to the agent");
        asked.chosen = Some(option.option_id.clone());
        self.send_answer(id);
    }

    /// Answers the question `id` as cancelled (ACP's `cancelled` outcome):
    /// nothing will answer it, so the agent is not left waiting.
    fn cancel_ask(&mut self, id: &str) {
        if let Some(Asked { rpc: Some(rpc), .. }) = self.asks.remove(id) {
            self.conn.respond(&self.agent, &rpc, json!({ "outcome": { "outcome": "cancelled" } }));
        }
    }

    /// Sends the chosen option to this connection's copy of the question, if
    /// both are there. The question stays until the host takes it back
    /// (`$/cancel_request`), so an answer a dropped connection lost goes
    /// again to the copy the host sends after the reconnect.
    fn send_answer(&mut self, id: &str) {
        let Some(asked) = self.asks.get(id) else { return };
        if let (Some(rpc), Some(option_id)) = (&asked.rpc, &asked.chosen) {
            let outcome = json!({ "outcome": { "outcome": "selected", "optionId": option_id } });
            self.conn.respond(&self.agent, rpc, outcome);
        }
    }

    /// The host took back this connection's copy of a request
    /// (`$/cancel_request`): answered, here or elsewhere, or cancelled. Its
    /// card is no longer waiting on the run.
    async fn withdrawn(&mut self, rpc: &Value) {
        let Some(id) = self.asks.iter().find(|(_, a)| a.rpc.as_ref() == Some(rpc)).map(|(id, _)| id.clone()) else {
            return;
        };
        let answered_here = self.asks.remove(&id).is_some_and(|a| a.chosen.is_some());
        if !answered_here {
            if let Some(channels) = self.req.ask_channels.as_ref() {
                channels.lock().await.remove(&id);
            }
            info!(request_id = %id, "linked: the question was answered elsewhere");
        }
    }

    /// The turn ended: its last cards, then how it ended as the owner reads
    /// it.
    async fn ended(&mut self, ended: TurnUpdate) -> Result<(), String> {
        self.tools.flush();
        self.cards().await;
        if let Some(error) = &ended.error {
            return Err(turn::plain(&self.name, error));
        }
        match ended.stop_reason {
            // The agent acknowledged the cancel: its own record of the
            // session is intact, so the session is kept — a "Stop… now do
            // X" resumes with its context, not a fresh session.
            Some(StopReason::Cancelled) => Err("Cancelled".to_owned()),
            Some(StopReason::Refusal) => Err(format!("{} declined to do that.", self.name)),
            _ => {
                if let Some(usage) = &ended.usage {
                    let usage = UsageInfo {
                        input_tokens: usage.all_input() as i32,
                        output_tokens: usage.output_tokens as i32,
                        ..UsageInfo::default()
                    };
                    let _ = self.tx.send(StreamEvent::usage(usage)).await;
                }
                let _ = self.tx.send(StreamEvent::done()).await;
                Ok(())
            }
        }
    }

    /// The connection dropped mid-turn: the bot reached again, the session
    /// loaded, and what this turn missed read from its record (spec §12).
    /// The turn's end, when it ended while Nebo was away.
    async fn reconnect(&mut self, answers: &mpsc::Sender<(String, String)>, cancel: &CancellationToken) -> Result<Option<TurnUpdate>, String> {
        for ask in self.asks.values_mut() {
            ask.rpc = None;
        }
        info!(bot_id = %self.bot_id, session = %self.session, "linked: the connection dropped mid-turn; reconnecting");
        for wait in RECONNECT {
            tokio::select! {
                _ = tokio::time::sleep(reconnect_wait(wait)) => {}
                _ = cancel.cancelled() => return Err("Cancelled".to_owned()),
            }
            self.conn = match self.provider.open(&self.route, &self.bot_id).await {
                Ok(conn) => conn,
                Err(Unreached::SignedOut) => return Err(format!("Sign in to NeboAI to reach {}.", self.name)),
                Err(_) => continue,
            };
            match self.reattach(answers).await {
                Ok(ended) => return Ok(ended),
                Err(Some(message)) => return Err(message),
                Err(None) => continue,
            }
        }
        Err(self.offline())
    }

    /// The session loaded on a new connection, and this turn caught up from
    /// its record. `Err(None)`: this connection failed too; try again.
    async fn reattach(&mut self, answers: &mpsc::Sender<(String, String)>) -> Result<Option<TurnUpdate>, Option<String>> {
        let agent = self.agent.clone();
        let params = json!({ "protocolVersion": 1, "clientCapabilities": {} });
        let (answer, _) = self.conn.call(Some(&agent), "initialize", params).await.ok_or(None)?;
        answer.map_err(|e| Some(turn::plain(&self.name, &e)))?;
        let params = json!({ "sessionId": self.session, "cwd": self.cwd(), "mcpServers": [] });
        let (answer, replay) = self.conn.call(Some(&agent), "session/load", params).await.ok_or(None)?;
        answer.map_err(|e| Some(turn::plain(&self.name, &e)))?;
        let mut record: Vec<Value> = Vec::new();
        let mut latest: Option<TurnUpdate> = None;
        for frame in replay {
            match replayed(&frame, &agent, &self.session) {
                Replayed::Turn(update) => latest = Some(update),
                Replayed::Update(update) => record.push(update),
                Replayed::Other => {
                    self.frame(frame, answers).await.map_err(Some)?;
                }
            }
        }
        if self.turn.is_none() {
            let ours = latest.as_ref().is_some_and(|t| t.by.as_ref().is_some_and(|d| d.device_id == self.conn.device));
            match &latest {
                // The prompt arrived and its turn started while Nebo was away.
                Some(update) if ours && self.prompted => self.turn = Some(update.turn_id.clone()),
                // Still behind another turn: sent when that one ends.
                Some(update) if self.waiting && update.state == TurnState::Running => return Ok(None),
                // The prompt never arrived, or the turn it waited behind is
                // over: sent (again) on this connection.
                _ => {
                    self.send_prompt().map_err(Some)?;
                    return Ok(None);
                }
            }
        }
        let latest = latest.filter(|t| Some(t.turn_id.as_str()) == self.turn.as_deref());
        // This turn's updates are the record after its prompt (and before the
        // next turn's, when one began after it); those read before the drop
        // are skipped.
        let missed: Vec<Value> = this_turns(&record, latest.is_some()).iter().skip(self.seen).cloned().collect();
        for update in &missed {
            self.seen += 1;
            self.update(update).await;
        }
        info!(session = %self.session, missed = missed.len(), "linked: reconnected mid-turn");
        Ok(match latest {
            Some(latest) => (latest.state == TurnState::Ended).then_some(latest),
            // It ended while Nebo was away and another turn began: its end
            // is no longer the session's latest, and its answer is all in.
            None => Some(TurnUpdate {
                agent: self.agent.clone(),
                session_id: self.session.clone(),
                turn_id: self.turn.clone().unwrap_or_default(),
                state: TurnState::Ended,
                started_at: String::new(),
                by: None,
                stop_reason: Some(StopReason::EndTurn),
                error: None,
                usage: None,
            }),
        })
    }
}

/// The owner's answer an option of `kind` is: the three answers of every
/// permission card, the ones his spoken answer on a call takes.
fn answer_of(kind: &str) -> Option<&'static str> {
    match kind {
        "allow_once" => Some("this_once"),
        "allow_always" => Some("allow_always"),
        "reject_once" | "reject_always" => Some("no"),
        _ => None,
    }
}

/// The updates of a turn in a session's record: those after the prompt that
/// began it, the newest prompt when it is the `latest` turn, else the one
/// before the newest.
fn this_turns(record: &[Value], latest: bool) -> &[Value] {
    // Each prompt's first and one-past-last update.
    let mut prompts: Vec<(usize, usize)> = Vec::new();
    for (i, update) in record.iter().enumerate() {
        if update["sessionUpdate"] != "user_message_chunk" {
            continue;
        }
        match prompts.last_mut() {
            Some((_, end)) if *end == i => *end = i + 1,
            _ => prompts.push((i, i + 1)),
        }
    }
    match (latest, prompts.as_slice()) {
        (true, [.., (_, end)]) => &record[*end..],
        (false, [.., (_, end), (next, _)]) => &record[*end..*next],
        (true, []) => record,
        _ => &[],
    }
}

/// Why a conversation continues in a fresh session of its agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rotation {
    /// Its turn stopped answering ([`Stall`]).
    Stalled,
    /// It has used most of its context window ([`LONG_PERCENT`]).
    Long,
    /// It had no turn for [`IDLE_ROTATION`].
    Idle,
}

/// Whether the session recorded on `chat` should continue fresh at this
/// message (`now`, unix seconds): it sat idle, or it got long.
fn rotation_due(chat: &db::models::Chat, now: i64) -> Option<Rotation> {
    let idle = IDLE_ROTATION.as_secs() as i64;
    if chat.linked_turn_at.is_some_and(|at| now.saturating_sub(at) >= idle) {
        return Some(Rotation::Idle);
    }
    match (chat.linked_used_tokens, chat.linked_window_tokens) {
        (Some(used), Some(window)) if window > 0 && used.saturating_mul(100) >= window.saturating_mul(LONG_PERCENT) => Some(Rotation::Long),
        _ => None,
    }
}

/// Watches one turn for a stall, a heartbeat at a time: its agent's host
/// says the prompt is open and nothing else works in it (no call running,
/// no permission request waiting, none of the agent's processes working)
/// and nothing new has come from the agent in the session, held for the
/// window. Silence alone never counts: a build, a test run or a running call
/// is work the host sees, and anything the agent sends starts it over. No
/// word from the host is no evidence either way.
#[derive(Debug, Default)]
struct Stall {
    since: Option<std::time::Instant>,
    last_update: Option<String>,
}

impl Stall {
    /// Takes in one look at the agent's `host/status` entry (`status`) for
    /// `session`; true once the turn has been stalled for `window`.
    fn look(&mut self, status: Option<&AgentStatus>, session: &str, now: std::time::Instant, window: Duration) -> bool {
        let seen = status.and_then(|agent| {
            let s = agent.sessions.iter().find(|s| s.session_id == session)?;
            let quiet = agent.state == Life::Running
                && !agent.why.contains(&Working::Processes)
                && s.why.contains(&Working::Prompt)
                && !s.why.iter().any(|w| matches!(w, Working::Tool | Working::Permission));
            Some((quiet, s.last_update.clone()))
        });
        match seen {
            Some((true, last_update)) => {
                if last_update != self.last_update {
                    self.last_update = last_update;
                    self.since = Some(now);
                }
                let since = *self.since.get_or_insert(now);
                now.saturating_duration_since(since) >= window
            }
            Some((false, last_update)) => {
                self.last_update = last_update;
                self.since = None;
                false
            }
            None => {
                self.since = None;
                false
            }
        }
    }

    /// Something came from the agent in the session: it is not stalled.
    fn heard(&mut self) {
        self.since = None;
    }
}

/// The latest exchanges before the owner's newest message, word for word
/// (each message bounded): what a fresh session reads besides the summary.
fn latest_exchanges(messages: &[Message]) -> Option<String> {
    let owners = |m: &Message| m.role == "user" && !m.content.trim().starts_with("<system-reminder>");
    let newest = messages.iter().rposition(owners)?;
    let lines: Vec<String> = messages[..newest]
        .iter()
        .filter(|m| (owners(m) || m.role == "assistant") && !m.content.trim().is_empty())
        .map(|m| match m.role.as_str() {
            "user" => format!("Owner: {}", clip(m.content.trim(), TAIL_CHARS, false)),
            _ => format!("You: {}", clip(m.content.trim(), TAIL_CHARS, true)),
        })
        .collect();
    let tail = &lines[lines.len().saturating_sub(TAIL_MESSAGES)..];
    (!tail.is_empty()).then(|| tail.join("\n\n"))
}

/// Whether the owner's message is the command `name` (`/name`, then any
/// words for it).
fn is_command(message: &str, name: &str) -> bool {
    message
        .trim()
        .strip_prefix('/')
        .and_then(|rest| rest.split_whitespace().next())
        .is_some_and(|command| command.eq_ignore_ascii_case(name))
}

/// Now, in unix seconds.
fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or_default()
}

/// What a linked agent's host is called in a status read: the employee
/// reading about it knows which computer that is.
const COMPUTER: &str = "its computer";

fn no_answer() -> String {
    "its computer did not answer".to_owned()
}

/// The agent `agent_id` names in a host's `host/agents` listing: by its
/// id, or by the one form every agent id now takes.
fn listed_agent<'a>(listed: &'a [Value], agent_id: &str) -> Option<&'a Value> {
    let canonical = link_core::roster::agent_id(agent_id);
    listed
        .iter()
        .find(|a| a["id"] == agent_id)
        .or_else(|| listed.iter().find(|a| a["id"] == canonical.as_str()))
}

/// One frame a `session/load` replayed, for the session `session` of
/// `agent`: its turn notice, one of its recorded updates, or anything else.
enum Replayed {
    Turn(TurnUpdate),
    Update(Value),
    Other,
}

fn replayed(frame: &Value, agent: &str, session: &str) -> Replayed {
    if frame.get("agent").is_none() && frame["method"] == "host/turn" {
        return match serde_json::from_value::<TurnUpdate>(frame["params"].clone()) {
            Ok(update) if update.agent == agent && update.session_id == session => Replayed::Turn(update),
            _ => Replayed::Other,
        };
    }
    if frame["agent"] == agent && frame["acp"]["method"] == "session/update" && frame["acp"]["params"]["sessionId"] == session {
        return Replayed::Update(frame["acp"]["params"]["update"].clone());
    }
    Replayed::Other
}

/// What a linked agent is doing in one session, as its record says: the
/// conversation's title, the owner's request its latest turn works on, that
/// turn's tool calls and its latest words. Bounded: the newest
/// [`WORK_CALLS`] calls, and the last [`WORK_CHARS`] characters of the
/// request and of the words, each cut named where it is cut.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionWork {
    pub title: Option<String>,
    /// The request the latest turn works on.
    pub prompt: Option<String>,
    /// The latest turn's tool calls, oldest first: what the agent shows for
    /// each, and where it is ("running", "done", "failed", "about to run").
    pub calls: Vec<(String, String)>,
    /// How many earlier calls of the turn are not in `calls`.
    pub earlier_calls: usize,
    /// What the agent said last in that turn.
    pub said: Option<String>,
    /// When the running turn started (RFC 3339); `None` when no turn runs.
    pub running_since: Option<String>,
}

/// The newest calls a [`SessionWork`] carries.
pub const WORK_CALLS: usize = 5;
/// The most of a request or of the latest words a [`SessionWork`] carries.
pub const WORK_CHARS: usize = 400;

/// A session's work from its record (the `session/update`s a load
/// replays, oldest first) and its running turn, if one runs.
fn summarise(record: &[Value], running: Option<&TurnUpdate>) -> SessionWork {
    let parsed: Vec<protocol::Update> = record
        .iter()
        .filter_map(|u| protocol::update(&json!({ "sessionId": "", "update": u })).map(|(_, p)| p))
        .collect();
    let mut work = SessionWork {
        running_since: running.map(|t| t.started_at.clone()).filter(|s| !s.is_empty()),
        ..SessionWork::default()
    };
    work.title = parsed.iter().rev().find_map(|u| match u {
        protocol::Update::Title(t) if !t.trim().is_empty() => Some(t.trim().to_owned()),
        _ => None,
    });
    // The latest request: the last run of the owner's message chunks.
    let last_user = parsed.iter().rposition(|u| matches!(u, protocol::Update::UserText { .. }));
    let after = match last_user {
        Some(end) => {
            let start = parsed[..=end]
                .iter()
                .rposition(|u| !matches!(u, protocol::Update::UserText { .. }))
                .map_or(0, |i| i + 1);
            let text: String = parsed[start..=end]
                .iter()
                .filter_map(|u| match u {
                    protocol::Update::UserText { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            work.prompt = bounded(text.trim(), false);
            &parsed[end + 1..]
        }
        None => &parsed[..],
    };
    let mut calls: Vec<(String, String, String)> = Vec::new();
    let mut said = String::new();
    for u in after {
        match u {
            protocol::Update::ToolCall(call) | protocol::Update::ToolCallUpdate(call) => {
                let label = call.title.clone().or_else(|| call.name.clone()).or_else(|| call.kind.clone()).filter(|l| !l.trim().is_empty());
                let status = call.status.map(|s| match s {
                    protocol::ToolStatus::Pending => "about to run",
                    protocol::ToolStatus::InProgress => "running",
                    protocol::ToolStatus::Completed => "done",
                    protocol::ToolStatus::Failed => "failed",
                });
                match calls.iter_mut().find(|(id, _, _)| *id == call.id) {
                    Some((_, l, s)) => {
                        if let Some(label) = label {
                            *l = label;
                        }
                        if let Some(status) = status {
                            *s = status.to_owned();
                        }
                    }
                    None => calls.push((
                        call.id.clone(),
                        label.unwrap_or_else(|| "a tool".to_owned()),
                        status.unwrap_or("running").to_owned(),
                    )),
                }
            }
            protocol::Update::AgentText { text, .. } => said.push_str(text),
            _ => {}
        }
    }
    work.earlier_calls = calls.len().saturating_sub(WORK_CALLS);
    work.calls = calls.into_iter().skip(work.earlier_calls).map(|(_, l, s)| (l, s)).collect();
    work.said = bounded(said.trim(), true);
    work
}

/// `text` bounded to [`WORK_CHARS`] ([`clip`]); `None` when empty.
fn bounded(text: &str, keep_end: bool) -> Option<String> {
    (!text.is_empty()).then(|| clip(text, WORK_CHARS, keep_end))
}

/// `text` bounded to `max` characters: its end when `keep_end` (the latest
/// words), else its start; the cut is said where it is.
fn clip(text: &str, max: usize, keep_end: bool) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    if keep_end {
        let tail: String = text.chars().skip(count - max).collect();
        format!("… {tail}")
    } else {
        let head: String = text.chars().take(max).collect();
        format!("{head} …")
    }
}

/// Why the linked bot could not be reached, as the owner reads it.
fn unreached(why: Unreached, name: &str) -> String {
    match why {
        Unreached::Offline => format!("Could not connect to {name}. Try again."),
        Unreached::SignedOut => format!("Sign in to NeboAI to reach {name}."),
        Unreached::Refused(message) => message,
    }
}

/// The owner's newest message: the newest user message that is not a
/// system reminder. Reminder rows are Nebo's context for Nebo's model (the
/// harness writes them as user rows opening with `<system-reminder>`) and
/// never reach another runtime.
fn owners_message(messages: &[Message]) -> Option<&str> {
    messages
        .iter()
        .rev()
        .filter(|m| m.role == "user")
        .map(|m| m.content.trim())
        .find(|c| !c.starts_with("<system-reminder>"))
        .filter(|c| !c.is_empty())
}

/// `<bot>/<agent>`, both present.
fn split(model: &str) -> Option<(&str, &str)> {
    let (bot, agent) = model.split_once('/')?;
    (!bot.is_empty() && !agent.is_empty() && !agent.contains('/')).then_some((bot, agent))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;

    /// The linked bot on "another computer".
    const BOT: &str = "5a137883-0000-4000-8000-000000000001";
    /// This bot's id, under which Nebo hosts this computer's agents.
    const SELF: &str = "5e1f0000-0000-4000-8000-000000000002";

    /// Not a test when the harness runs it: the ACP agent a hire starts (this
    /// test binary again, with `NEBO_FAKE_ACP` naming the file it writes what
    /// it was told to, and `NEBO_FAKE_ACP_SCRIPT` what it does with a prompt):
    ///
    /// - `git` (the default): asks before it runs `git status`, then runs it.
    /// - `turn`: thinks, says, runs `ls`, says, and ends with its usage.
    /// - `ask`: asks in its own words whether to run `rm -rf build`.
    /// - `hang`: says it is working until it is cancelled.
    /// - `deaf`: says it is working, then never acknowledges a cancel.
    /// - `dies`: says it is working, then the process itself exits.
    /// - `coder`: offers a bypass mode; in it, runs `git status` without
    ///   asking, else asks first as `git` does.
    /// - `folders`: takes the host's HTTP MCP server; `where` says the folder
    ///   its session works in (and the handoff its prompt started with),
    ///   `work in <folder>` calls the host's `move_to_folder`.
    /// - `stall`: its first session is wedged: every prompt there opens a
    ///   call that stays pending (a model's stream stopped mid-call), then
    ///   nothing; a cancel is acknowledged, and the session stays wedged,
    ///   in this process or the next (it loads sessions). Every other
    ///   session answers `Fresh here.`
    /// - `busy`: a call running (`in_progress`), then nothing until
    ///   cancelled: quiet, but working.
    /// - `rotate`: each new session its own (`s-1`, `s-2`, …), offering the
    ///   commands `compact` and `review`; a prompt is answered `In
    ///   <session>.` with the context it used: most of it for `a long one`,
    ///   little otherwise; `/compact` is answered `Compacted.`
    #[test]
    fn fake_acp_agent() {
        use std::io::{BufRead, Write};
        let Ok(told) = std::env::var("NEBO_FAKE_ACP") else {
            return;
        };
        let script = std::env::var("NEBO_FAKE_ACP_SCRIPT").unwrap_or_else(|_| "git".to_owned());
        let note = |line: Value| {
            let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&told).unwrap();
            writeln!(file, "{line}").unwrap();
        };
        let send = |frame: Value| {
            let mut out = std::io::stdout().lock();
            writeln!(out, "{frame}").unwrap();
            out.flush().unwrap();
        };
        let update = |update: Value| {
            send(json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": "s-1", "update": update } }));
        };
        let say = |text: &str| update(json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": text } }));
        // The harness printed "test ... " without a newline: end that line,
        // so every frame is a line of its own.
        send(Value::Null);
        let mut prompt_id = Value::Null;
        // The session's mode, as `session/set_mode` last set it.
        let mut mode = "default".to_owned();
        // Each session's folder and the host's MCP server it was given.
        let mut sessions: HashMap<String, (String, String)> = HashMap::new();
        for line in std::io::stdin().lock().lines() {
            let Ok(message) = serde_json::from_str::<Value>(&line.unwrap()) else {
                continue;
            };
            let id = message["id"].clone();
            let reply = |result: Value| send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
            match message["method"].as_str() {
                Some("initialize") => reply(json!({
                    "protocolVersion": 1,
                    "agentCapabilities": match script.as_str() {
                        "folders" => json!({ "loadSession": false, "mcpCapabilities": { "http": true } }),
                        "stall" => json!({ "loadSession": true }),
                        _ => json!({ "loadSession": false }),
                    },
                    "agentInfo": { "name": "fake-acp" },
                })),
                // Its sessions are numbered by every one it made, in any of
                // its processes (`told` outlives them).
                Some("session/new") if script == "stall" || script == "rotate" => {
                    let made = self::told(std::path::Path::new(&told)).iter().filter(|t| t.get("new").is_some()).count();
                    note(json!({ "new": message["params"]["cwd"] }));
                    let session = format!("s-{}", made + 1);
                    reply(json!({ "sessionId": session }));
                    // Just after it opens a session, it says what commands
                    // the session offers.
                    if script == "rotate" {
                        emit(&json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": session, "update": {
                            "sessionUpdate": "available_commands_update",
                            "availableCommands": [{ "name": "compact", "description": "Compact the conversation" }, { "name": "review", "description": "Review" }],
                        } } }));
                    }
                }
                Some("session/load") if script == "stall" => {
                    note(json!({ "load": message["params"]["sessionId"] }));
                    reply(json!({}));
                }
                Some("session/prompt") if script == "stall" || script == "rotate" => {
                    note(json!({ "prompt": message["params"]["prompt"] }));
                    let session = message["params"]["sessionId"].as_str().unwrap_or("").to_owned();
                    let update_in = |update: Value| emit(&json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": session, "update": update } }));
                    let text = message["params"]["prompt"].as_array().and_then(|p| p.last()).and_then(|b| b["text"].as_str()).unwrap_or("").to_owned();
                    if script == "stall" && session == "s-1" {
                        prompt_id = id.clone();
                        update_in(json!({ "sessionUpdate": "tool_call", "toolCallId": "call_1", "title": "Read", "kind": "read", "status": "pending" }));
                    } else if script == "stall" {
                        update_in(json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "Fresh here." } }));
                        reply(json!({ "stopReason": "end_turn" }));
                    } else if text.starts_with("/compact") {
                        update_in(json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "Compacted." } }));
                        reply(json!({ "stopReason": "end_turn" }));
                    } else {
                        let used = if text == "a long one" { 150_000 } else { 1_000 };
                        update_in(json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": format!("In {session}.") } }));
                        update_in(json!({ "sessionUpdate": "usage_update", "used": used, "size": 200_000 }));
                        reply(json!({ "stopReason": "end_turn" }));
                    }
                }
                Some("session/new") if script == "folders" => {
                    note(json!({ "new": message["params"]["cwd"] }));
                    let session = format!("s-{}", sessions.len() + 1);
                    let url = message["params"]["mcpServers"]
                        .as_array()
                        .and_then(|servers| servers.iter().find(|s| s["type"] == "http" && s["name"] == "host"))
                        .and_then(|s| s["url"].as_str())
                        .unwrap_or("")
                        .to_owned();
                    let cwd = message["params"]["cwd"].as_str().unwrap_or("").to_owned();
                    sessions.insert(session.clone(), (cwd, url));
                    reply(json!({ "sessionId": session }));
                }
                Some("session/prompt") if script == "folders" => {
                    note(json!({ "prompt": message["params"]["prompt"] }));
                    let session = message["params"]["sessionId"].as_str().unwrap_or("").to_owned();
                    let texts: Vec<String> = message["params"]["prompt"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|b| b["text"].as_str().map(str::to_owned))
                        .collect();
                    let text = texts.last().cloned().unwrap_or_default();
                    let (cwd, url) = sessions.get(&session).cloned().unwrap_or_default();
                    let say_in = |text: &str| {
                        emit(&json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": session,
                            "update": { "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": text } } } }))
                    };
                    if let Some(folder) = text.strip_prefix("work in ") {
                        emit(&json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": session,
                            "update": { "sessionUpdate": "tool_call", "toolCallId": "call_move", "title": "move_to_folder", "kind": "other", "status": "in_progress" } } }));
                        // The host answers the tool after it started the
                        // new session here: the call runs beside this loop.
                        let (folder, id) = (folder.to_owned(), id.clone());
                        std::thread::spawn(move || {
                            let (status, said) = match call_move(&url, &folder, &format!("Was working in {cwd}.")) {
                                Ok(_) => ("completed", "Moved.".to_owned()),
                                Err(why) => ("failed", format!("Couldn't move: {why}")),
                            };
                            let update = |update: Value| emit(&json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": session, "update": update } }));
                            update(json!({ "sessionUpdate": "tool_call_update", "toolCallId": "call_move", "status": status }));
                            update(json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": said } }));
                            emit(&json!({ "jsonrpc": "2.0", "id": id, "result": { "stopReason": "end_turn" } }));
                        });
                    } else {
                        say_in(&format!("Working in {cwd}."));
                        if texts.len() > 1 {
                            say_in(&format!(" Handoff: {}", texts[0]));
                        }
                        reply(json!({ "stopReason": "end_turn" }));
                    }
                }
                Some("session/new") => {
                    note(json!({ "new": message["params"]["cwd"] }));
                    let mut modes = vec![
                        json!({ "id": "default", "name": "Default", "_meta": { "kind": "standard" } }),
                        json!({ "id": "acceptEdits", "name": "Accept edits", "_meta": { "kind": "standard" } }),
                    ];
                    if script == "turn" || script == "coder" {
                        modes.push(json!({ "id": "bypassPermissions", "name": "Bypass", "_meta": { "kind": "full_access" } }));
                    }
                    reply(json!({ "sessionId": "s-1", "modes": { "currentModeId": "default", "availableModes": modes } }));
                }
                Some("session/set_mode") => {
                    note(json!({ "mode": message["params"]["modeId"] }));
                    mode = message["params"]["modeId"].as_str().unwrap_or_default().to_owned();
                    reply(json!({}));
                }
                // "deaf": never acknowledges a cancel — the wedge case
                // Nebo's own CANCEL_TIMEOUT has to give up on.
                Some("session/cancel") if script != "deaf" => {
                    note(json!({ "cancel": true }));
                    send(json!({ "jsonrpc": "2.0", "id": prompt_id, "result": { "stopReason": "cancelled" } }));
                }
                Some("session/cancel") => {
                    note(json!({ "cancel": true }));
                }
                Some("session/prompt") => {
                    note(json!({ "prompt": message["params"]["prompt"] }));
                    prompt_id = id.clone();
                    match script.as_str() {
                        "turn" => {
                            update(json!({ "sessionUpdate": "agent_thought_chunk", "content": { "type": "text", "text": "hmm" } }));
                            say("Listing ");
                            update(json!({ "sessionUpdate": "tool_call", "toolCallId": "call_1", "title": "terminal", "kind": "execute",
                                "status": "in_progress", "rawInput": { "command": "ls" } }));
                            update(json!({ "sessionUpdate": "tool_call_update", "toolCallId": "call_1", "status": "completed",
                                "content": [{ "type": "content", "content": { "type": "text", "text": "a b" } }] }));
                            say("done.");
                            reply(json!({ "stopReason": "end_turn", "usage": { "inputTokens": 12, "outputTokens": 5 } }));
                        }
                        "ask" => send(json!({ "jsonrpc": "2.0", "id": 900, "method": "session/request_permission", "params": {
                            "sessionId": "s-1",
                            "toolCall": { "toolCallId": "req-9", "title": "rm -rf build", "kind": "execute", "status": "pending" },
                            "options": [
                                { "optionId": "once", "name": "Allow once", "kind": "allow_once" },
                                { "optionId": "always", "name": "Always allow", "kind": "allow_always" },
                                { "optionId": "deny", "name": "Deny", "kind": "reject_once" },
                            ],
                            "_meta": { "nebo/words": { "question": "Run `rm -rf build`?", "summary": "run `rm -rf build`",
                                "labels": ["Allow once", "Always allow", "Deny"] } },
                        } })),
                        "hang" | "deaf" => say("Working"),
                        "busy" => update(json!({ "sessionUpdate": "tool_call", "toolCallId": "call_1", "title": "cargo build", "kind": "execute",
                            "status": "in_progress", "rawInput": { "command": "cargo build" } })),
                        // The Mac-mini incident: the process itself dies
                        // mid-turn, not just the connection — a crash, not a
                        // network blip.
                        "dies" => {
                            say("Working");
                            std::process::exit(1);
                        }
                        "coder" if mode == "bypassPermissions" => {
                            update(json!({ "sessionUpdate": "tool_call", "toolCallId": "call_1", "title": "git status", "kind": "execute",
                                "status": "in_progress", "rawInput": { "command": "git status" } }));
                            update(json!({ "sessionUpdate": "tool_call_update", "toolCallId": "call_1", "status": "completed",
                                "content": [{ "type": "content", "content": { "type": "text", "text": "nothing to commit" } }] }));
                            say("Ran it.");
                            reply(json!({ "stopReason": "end_turn" }));
                        }
                        _ => {
                            say("Checking.");
                            send(json!({ "jsonrpc": "2.0", "id": 900, "method": "session/request_permission", "params": {
                                "sessionId": "s-1",
                                "toolCall": { "toolCallId": "call_1", "title": "git status", "kind": "execute", "status": "pending", "rawInput": { "command": "git status" } },
                                "options": [
                                    { "optionId": "allow", "name": "Allow", "kind": "allow_once" },
                                    { "optionId": "reject", "name": "Reject", "kind": "reject_once" },
                                ],
                            } }));
                        }
                    }
                }
                None if id == json!(900) => {
                    note(json!({ "answer": message["result"]["outcome"] }));
                    if script == "ask" {
                        let chosen = match message["result"]["outcome"]["optionId"].as_str() {
                            Some("once") => "Allow once",
                            Some("always") => "Always allow",
                            _ => "Deny",
                        };
                        say(&format!("answered: {chosen}"));
                        send(json!({ "jsonrpc": "2.0", "id": prompt_id, "result": { "stopReason": "end_turn" } }));
                        continue;
                    }
                    update(json!({ "sessionUpdate": "tool_call_update", "toolCallId": "call_1", "status": "completed",
                        "content": [{ "type": "content", "content": { "type": "text", "text": "nothing to commit" } }] }));
                    say("Ran it.");
                    send(json!({ "jsonrpc": "2.0", "id": prompt_id, "result": { "stopReason": "end_turn", "usage": { "inputTokens": 7, "outputTokens": 3 } } }));
                }
                _ => {}
            }
        }
    }

    /// One frame on the fake agent's stdout, from any of its threads.
    fn emit(frame: &Value) {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        writeln!(out, "{frame}").unwrap();
        out.flush().unwrap();
    }

    /// The host's `move_to_folder`, called as an MCP client does over
    /// Streamable HTTP: the tool's text, or its error.
    fn call_move(url: &str, folder: &str, handoff: &str) -> Result<String, String> {
        use std::io::{Read, Write};
        let rest = url.strip_prefix("http://").ok_or("the host gave no tools")?;
        let (authority, path) = rest.split_once('/').ok_or("no path")?;
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "move_to_folder", "arguments": { "path": folder, "handoff": handoff } } })
        .to_string();
        let mut stream = std::net::TcpStream::connect(authority).map_err(|e| e.to_string())?;
        write!(
            stream,
            "POST /{path} HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .map_err(|e| e.to_string())?;
        let mut response = String::new();
        stream.read_to_string(&mut response).map_err(|e| e.to_string())?;
        let answer: Value = serde_json::from_str(response.split_once("\r\n\r\n").ok_or("no answer")?.1.trim()).map_err(|e| e.to_string())?;
        let text = answer["result"]["content"][0]["text"].as_str().unwrap_or("").to_owned();
        match answer["result"]["isError"].as_bool() {
            Some(false) => Ok(text),
            _ => Err(text),
        }
    }

    /// How a hire starts the fake agent running `script`.
    fn fake_acp(told: &std::path::Path, script: &str) -> nebo_runtimes::RuntimeCommand {
        nebo_runtimes::RuntimeCommand {
            program: std::env::current_exe().unwrap().to_string_lossy().into_owned(),
            args: ["providers::linked::tests::fake_acp_agent", "--exact", "--nocapture", "--test-threads=1"]
                .map(String::from)
                .to_vec(),
            env: vec![
                ("NEBO_FAKE_ACP".into(), told.to_string_lossy().into_owned()),
                ("NEBO_FAKE_ACP_SCRIPT".into(), script.into()),
            ],
        }
    }

    /// What the fake agent was told, in order.
    fn told(path: &std::path::Path) -> Vec<Value> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    /// A computer's host, as Nebo hosts one: `bot` its bot id, its record in
    /// `root`, and its nebo-link daemon's state (none) there too.
    fn host_at(root: &std::path::Path, bot: &'static str) -> Arc<LocalHost> {
        LocalHost::open(Arc::new(move || Some(bot.to_owned())), root.join("link"), root.join("home"), Some(root.join("nebo-link"))).unwrap()
    }

    /// Nebo's own host (and keys) for the tests.
    fn local_host(root: &std::path::Path) -> Arc<LocalHost> {
        host_at(root, SELF)
    }

    /// What the fake hub saw.
    #[derive(Default)]
    struct Hub {
        bearers: Vec<String>,
        /// The sockets it relays, to drop them.
        sockets: Vec<tokio::task::AbortHandle>,
        /// The listener's accept loop, to take the whole hub down (a bot
        /// truly unreachable, not just this one connection).
        accept_loop: Option<tokio::task::AbortHandle>,
        /// Answers every socket 502, as NeboAI did when its relay blipped.
        failing: bool,
    }

    impl Hub {
        fn drop_sockets(&mut self) {
            for socket in self.sockets.drain(..) {
                socket.abort();
            }
        }

        /// The bot is gone, not just this connection: no new connection to
        /// it succeeds either, so a reconnect can never recover.
        fn shut_down(&mut self) {
            self.drop_sockets();
            if let Some(accept_loop) = self.accept_loop.take() {
                accept_loop.abort();
            }
        }
    }

    /// NeboAI's side of a linked bot, as a test sees it: `/t/<bot>/oal`
    /// carried to the bot's OAL host as its tunnel would (the hub relays the
    /// WebSocket's bytes and reads none of them), and the bot's own
    /// `/_link/oal/pair`.
    async fn fake_hub(oal: Arc<OalHost>) -> (String, Arc<Mutex<Hub>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let hub = Arc::new(Mutex::new(Hub::default()));
        let seen = hub.clone();
        let accept_loop = tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let (oal, hub) = (oal.clone(), seen.clone());
                tokio::spawn(async move { serve_hub(stream, oal, hub).await });
            }
        });
        hub.lock().unwrap().accept_loop = Some(accept_loop.abort_handle());
        (url, hub)
    }

    async fn serve_hub(mut stream: TcpStream, oal: Arc<OalHost>, hub: Arc<Mutex<Hub>>) {
        let mut head = [0u8; 2048];
        let n = stream.peek(&mut head).await.unwrap();
        let head = String::from_utf8_lossy(&head[..n]).into_owned();
        let target = head.split_whitespace().nth(1).unwrap_or("").to_owned();
        if target == format!("/t/{BOT}/oal") && hub.lock().unwrap().failing {
            hub.lock().unwrap().bearers.push("refused".to_owned());
            let _ = stream.read(&mut [0u8; 2048]).await;
            let _ = stream.write_all(b"HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
            return;
        }
        if target == format!("/t/{BOT}/oal") {
            let seen = hub.clone();
            let ws = tokio_tungstenite::accept_hdr_async(
                stream,
                move |req: &tokio_tungstenite::tungstenite::handshake::server::Request,
                      mut resp: tokio_tungstenite::tungstenite::handshake::server::Response| {
                    let bearer = req.headers().get(AUTHORIZATION).and_then(|v| v.to_str().ok()).unwrap_or("").to_owned();
                    seen.lock().unwrap().bearers.push(bearer);
                    resp.headers_mut().insert("sec-websocket-protocol", "oal".parse().unwrap());
                    Ok(resp)
                },
            )
            .await
            .unwrap();
            let task = tokio::spawn(async move { oal.serve(oal_host::wire::websocket(ws), oal_host::Via::Tunnel).await });
            hub.lock().unwrap().sockets.push(task.abort_handle());
            return;
        }
        // REST: read the whole request, answer by path.
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let n = stream.read(&mut chunk).await.unwrap();
            buf.extend_from_slice(&chunk[..n]);
            if n == 0 || String::from_utf8_lossy(&buf).contains("\r\n\r\n") {
                break;
            }
        }
        let bearer = head
            .lines()
            .find_map(|l| l.to_lowercase().strip_prefix("authorization:").map(|v| v.trim().to_owned()))
            .unwrap_or_default();
        let (status, body) = if target == format!("/t/{BOT}/_link/oal/pair") && head.starts_with("POST") {
            hub.lock().unwrap().bearers.push(bearer);
            let code = oal.pairing_code().await.unwrap();
            ("200 OK", json!({ "code": code.to_string(), "hostId": BOT }))
        } else {
            ("404 Not Found", json!({ "error": "not found" }))
        };
        let body = body.to_string();
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        stream.shutdown().await.ok();
    }

    /// A linked bot on another computer hosting the fake agent running
    /// `script` (as `claude-code`), reached through a fake hub; and the
    /// provider Nebo reaches it with.
    struct Remote {
        _root: tempfile::TempDir,
        bot: Arc<LocalHost>,
        hub: Arc<Mutex<Hub>>,
        told: std::path::PathBuf,
        store: Arc<db::Store>,
        provider: LinkedProvider,
    }

    async fn remote(script: &str) -> Remote {
        let root = tempfile::tempdir().unwrap();
        let told = root.path().join("told.jsonl");
        let bot = host_at(&root.path().join("bot"), BOT);
        let hosted = bot.host(nebo_runtimes::acp::Agent::ClaudeCode, fake_acp(&told, script)).await.unwrap();
        assert_eq!(hosted.id, "claude-code");
        let (url, hub) = fake_hub(bot.oal().unwrap()).await;
        let nebo = local_host(&root.path().join("nebo"));
        let (store, _) = store_in(root.path());
        let relay = Relay::Hub {
            api_url: url,
            token: Arc::new(|| Some("bot-jwt".to_owned())),
        };
        let provider = LinkedProvider::new(relay, store.clone(), Some(nebo), "Nebo on test");
        Remote {
            _root: root,
            bot,
            hub,
            told,
            store,
            provider,
        }
    }

    fn store_in(dir: &std::path::Path) -> (Arc<db::Store>, std::path::PathBuf) {
        let path = dir.join("linked.db");
        let store = db::Store::new(&path.to_string_lossy()).unwrap();
        store
            .create_agent("emp-1", Some("linked"), "Danny", "", "---\nname: Danny\n---\n", "{}", None, None)
            .unwrap();
        store.create_chat("chat-1", "First").unwrap();
        (Arc::new(store), path)
    }

    fn request(prompt: &str, chat_id: &str, model: &str) -> ChatRequest {
        ChatRequest {
            messages: vec![
                Message {
                    role: "user".into(),
                    content: "earlier".into(),
                    ..Default::default()
                },
                Message {
                    role: "assistant".into(),
                    content: "ok".into(),
                    ..Default::default()
                },
                Message {
                    role: "user".into(),
                    content: prompt.into(),
                    ..Default::default()
                },
            ],
            model: model.into(),
            chat_id: chat_id.into(),
            system: "NEVER SENT".into(),
            ..ChatRequest::new(RequestTrace {
                agent_id: "emp-1".into(),
                ..RequestTrace::new("agent_turn")
            })
        }
    }

    fn remote_model() -> String {
        format!("{BOT}/claude-code")
    }

    async fn collect(mut rx: EventReceiver) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        while let Some(e) = tokio::time::timeout(Duration::from_secs(60), rx.recv()).await.expect("the turn went on") {
            events.push(e);
        }
        events
    }

    /// Events up to the first ask, which is returned with them.
    async fn until_ask(rx: &mut EventReceiver) -> (Vec<StreamEvent>, StreamEvent) {
        let mut before = Vec::new();
        loop {
            let event = tokio::time::timeout(Duration::from_secs(60), rx.recv()).await.unwrap().unwrap();
            if event.event_type == StreamEventType::AskRequest {
                return (before, event);
            }
            assert_ne!(event.event_type, StreamEventType::Error, "{:?}", event.error);
            before.push(event);
        }
    }

    /// What the harness would tell a fresh session, as a test fixes it.
    struct Context;

    #[async_trait]
    impl LinkedContext for Context {
        fn briefing(&self) -> String {
            "BRIEFING".to_owned()
        }

        async fn summary(&self) -> Option<String> {
            Some("SUMMARY".to_owned())
        }
    }

    /// `req` with the harness's context for a fresh session.
    fn briefed(mut req: ChatRequest) -> ChatRequest {
        req.linked_context = Some(LinkedContextRef(Arc::new(Context)));
        req
    }

    /// The line the owner reads when the conversation continues fresh.
    const NOTICE: &str = "Continuing in a fresh Claude Code session with a summary of this conversation.";

    /// The handoff a fresh session that carries the conversation on starts
    /// with: the briefing, Nebo's summary, the latest exchanges word for
    /// word (the request's `earlier` and `ok`).
    fn assert_handoff(lead: &str) {
        assert!(lead.starts_with("BRIEFING\n\nThis conversation carries on from an earlier session."), "{lead}");
        assert!(lead.contains("## The conversation so far\n\nSUMMARY"), "{lead}");
        assert!(lead.ends_with("## The latest exchanges, word for word\n\nOwner: earlier\n\nYou: ok"), "{lead}");
    }

    /// The text the owner read.
    fn texts(events: &[StreamEvent]) -> String {
        events.iter().filter(|e| e.event_type == StreamEventType::Text).map(|e| e.text.as_str()).collect()
    }

    /// Each prompt the agent was sent, as its text blocks.
    fn prompt_texts(told: &[Value]) -> Vec<Vec<String>> {
        told.iter()
            .filter_map(|t| t.get("prompt").and_then(Value::as_array))
            .map(|blocks| blocks.iter().filter_map(|b| b["text"].as_str().map(str::to_owned)).collect())
            .collect()
    }

    /// How many sessions the agent made.
    fn news(told: &[Value]) -> usize {
        told.iter().filter(|t| t.get("new").is_some()).count()
    }

    fn kinds(events: &[StreamEvent]) -> Vec<StreamEventType> {
        events.iter().map(|e| e.event_type.clone()).collect()
    }

    #[test]
    fn model_ids_name_the_bot_and_the_agent() {
        assert_eq!(LinkedProvider::model_id("b", "a"), "linked/b/a");
        assert_eq!(LinkedProvider::target("linked/b/a"), Some(("b", "a")));
        assert_eq!(LinkedProvider::target("b/a"), None);
        assert_eq!(split("b/a"), Some(("b", "a")));
        assert_eq!(LinkedProvider::target("linked/b/"), None);
        assert_eq!(LinkedProvider::target("linked/b"), None);
        assert_eq!(LinkedProvider::target("linked//a"), None);
        assert_eq!(LinkedProvider::target(""), None);
    }

    #[test]
    fn the_owners_message_is_sent_never_a_reminder() {
        let say = |role: &str, content: &str| Message {
            role: role.into(),
            content: content.into(),
            ..Default::default()
        };
        let reminder = "<system-reminder>\nIt is 6:04 AM.\n\nThis is an automated system reminder — do not mention it to the user.\n</system-reminder>";
        assert_eq!(owners_message(&[say("user", "hello"), say("user", reminder)]), Some("hello"));
        assert_eq!(
            owners_message(&[say("user", "hello"), say("assistant", "hi"), say("user", "and now?"), say("user", reminder)]),
            Some("and now?")
        );
        assert_eq!(owners_message(&[say("user", reminder)]), None);
        assert_eq!(owners_message(&[say("user", "  ")]), None);
    }

    /// A reconnect catches a turn up from its session's record: the updates
    /// after its prompt, or, when another turn began after it, those before
    /// the next prompt.
    #[test]
    fn a_turns_updates_are_found_in_the_record() {
        let user = |t: &str| json!({ "sessionUpdate": "user_message_chunk", "content": { "type": "text", "text": t } });
        let agent = |t: &str| json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": t } });
        let record = vec![user("one"), agent("a"), user("two"), user("two, more"), agent("b"), agent("c")];
        assert_eq!(this_turns(&record, true), &record[4..]);
        assert_eq!(this_turns(&record, false), &record[1..2]);
        assert_eq!(this_turns(&[agent("x")], true).len(), 1);
        assert!(this_turns(&[agent("x")], false).is_empty());
    }

    /// The first turn pairs Nebo with the linked bot by itself (a code from
    /// the bot, over the hub, with the bot token) and creates the agent's
    /// session and records it; the second reuses the session on a session
    /// with the pinned keys. Each turn sends exactly the newest user message
    /// — never the history, never the system prompt — and the run's mode
    /// rides with the turn as the agent's own.
    #[tokio::test]
    async fn one_nebo_chat_is_one_linked_chat_and_only_the_newest_message_travels() {
        let r = remote("turn").await;
        let first = collect(r.provider.stream(&request("list the files", "chat-1", &remote_model())).await.unwrap()).await;
        assert_eq!(
            kinds(&first),
            vec![
                StreamEventType::Thinking,
                StreamEventType::Text,
                StreamEventType::ToolCall,
                StreamEventType::ToolResult,
                StreamEventType::Text,
                StreamEventType::Usage,
                StreamEventType::Done,
            ],
            "{:?}",
            first.iter().map(|e| (&e.text, &e.error)).collect::<Vec<_>>()
        );
        assert_eq!(first[0].text, "hmm");
        assert_eq!(first[1].text, "Listing ");
        let call = first[2].tool_call.as_ref().unwrap();
        assert_eq!((call.id.as_str(), call.name.as_str()), ("call_1", "terminal"));
        assert_eq!(call.input["command"], "ls");
        assert_eq!(first[3].text, "a b");
        assert_eq!(first[3].tool_call.as_ref().unwrap().id, "call_1");
        let usage = first[5].usage.as_ref().unwrap();
        assert_eq!((usage.input_tokens, usage.output_tokens), (12, 5));
        let chat = r.store.get_chat("chat-1").unwrap().unwrap();
        assert_eq!(chat.linked_chat_id.as_deref(), Some("s-1"));
        assert_eq!(chat.linked_agent_id.as_deref(), Some("claude-code"));

        let mut in_a_run = request("and now?", "chat-1", &remote_model());
        in_a_run.permission_mode = Some(types::permissions::Mode::FullAccess);
        let second = collect(r.provider.stream(&in_a_run).await.unwrap()).await;
        assert_eq!(second.last().unwrap().event_type, StreamEventType::Done);

        let bearers = r.hub.lock().unwrap().bearers.clone();
        assert_eq!(bearers.len(), 3, "a pairing code and two sockets: {bearers:?}");
        assert!(bearers.iter().all(|b| b.eq_ignore_ascii_case("Bearer bot-jwt")), "{bearers:?}");
        let devices = r.bot.oal().unwrap().devices();
        assert_eq!(devices.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["Nebo on test"], "Nebo paired once");
        let told = told(&r.told);
        let folder = r.bot.agents()[0].acp.workdir.to_string_lossy().into_owned();
        assert_eq!(
            told,
            vec![
                json!({ "new": folder }),
                json!({ "prompt": [{ "type": "text", "text": "list the files" }] }),
                json!({ "mode": "bypassPermissions" }),
                json!({ "prompt": [{ "type": "text", "text": "and now?" }] }),
            ],
            "one session; the run's mode on the second turn only; only the newest message"
        );
        let wire = serde_json::to_string(&told).unwrap();
        assert!(!wire.contains("earlier") && !wire.contains("NEVER SENT"), "{wire}");
    }

    /// An ask becomes an `ask_request` on the run's ask channels, in the
    /// agent's own words with its own options; the option answered there goes
    /// back as the option it names.
    #[tokio::test]
    async fn an_ask_round_trips_through_the_ask_channels() {
        let r = remote("ask").await;
        let channels: AskChannels = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let mut req = request("clean the build", "chat-1", &remote_model());
        req.ask_channels = Some(channels.clone());

        let mut rx = r.provider.stream(&req).await.unwrap();
        let (before, ask) = until_ask(&mut rx).await;
        assert!(before.is_empty(), "{:?}", kinds(&before));
        assert_eq!(ask.error.as_deref(), Some("req-9"), "the question's id");
        assert_eq!(ask.text, "Run `rm -rf build`?");
        let widgets = ask.widgets.as_ref().unwrap();
        assert_eq!(widgets[0]["options"], json!(["Allow once", "Always allow", "Deny"]));

        // The ask card's answer, through the ONE pathway.
        let sender = channels.lock().await.remove("req-9").expect("registered on the run");
        sender.send("Always allow".to_owned()).unwrap();

        let rest = collect(rx).await;
        assert_eq!(kinds(&rest), vec![StreamEventType::Text, StreamEventType::Done]);
        assert_eq!(rest[0].text, "answered: Always allow");
        let answers: Vec<Value> = told(&r.told).into_iter().filter_map(|t| t.get("answer").cloned()).collect();
        assert_eq!(answers, vec![json!({ "outcome": "selected", "optionId": "always" })]);
    }

    /// Every option the agent offered is on the card, each with the owner's
    /// answer it is; an answer given as the owner's word (a spoken "no" on
    /// his call) goes back as the option of that kind, in the same turn.
    #[tokio::test]
    async fn the_owners_answer_goes_back_as_the_option_of_its_kind() {
        let r = remote("ask").await;
        let channels: AskChannels = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let mut req = request("clean the build", "chat-1", &remote_model());
        req.ask_channels = Some(channels.clone());

        let mut rx = r.provider.stream(&req).await.unwrap();
        let (_, ask) = until_ask(&mut rx).await;
        let widget = &ask.widgets.as_ref().unwrap()[0];
        assert_eq!(widget["options"], json!(["Allow once", "Always allow", "Deny"]), "every option the agent offered");
        assert_eq!(widget["answers"], json!(["this_once", "allow_always", "no"]), "the owner's answer each one is");

        channels.lock().await.remove("req-9").expect("registered on the run").send("no".to_owned()).unwrap();
        let rest = collect(rx).await;
        assert_eq!(kinds(&rest), vec![StreamEventType::Text, StreamEventType::Done], "the same turn goes on");
        assert_eq!(rest[0].text, "answered: Deny");
        let told = told(&r.told);
        let answers: Vec<Value> = told.iter().filter_map(|t| t.get("answer").cloned()).collect();
        assert_eq!(answers, vec![json!({ "outcome": "selected", "optionId": "deny" })]);
        assert_eq!(told.iter().filter(|t| t.get("prompt").is_some()).count(), 1, "the turn was not sent again");
    }

    /// Full Access with an agent that has no no-prompt mode of its own: its
    /// ask never reaches the owner. Nebo allows it, once, and the turn goes
    /// on.
    #[tokio::test]
    async fn full_access_allows_an_agents_ask_itself() {
        let r = remote("ask").await;
        let channels: AskChannels = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let mut req = request("clean the build", "chat-1", &remote_model());
        req.permission_mode = Some(types::permissions::Mode::FullAccess);
        req.ask_channels = Some(channels.clone());

        let events = collect(r.provider.stream(&req).await.unwrap()).await;
        assert_eq!(kinds(&events), vec![StreamEventType::Text, StreamEventType::Done], "no card");
        assert_eq!(events[0].text, "answered: Allow once");
        assert!(channels.lock().await.is_empty(), "nothing waited on the owner");
        let told = told(&r.told);
        assert!(!told.iter().any(|t| t.get("mode").is_some()), "it has no full-access mode to switch to");
        let answers: Vec<Value> = told.iter().filter_map(|t| t.get("answer").cloned()).collect();
        assert_eq!(answers, vec![json!({ "outcome": "selected", "optionId": "once" })]);
    }

    /// The employee's mode is the agent's own from the next message. In Full
    /// Access the agent runs in its no-prompt mode and an ordinary call asks
    /// nobody; switched to Ask (no hire again), the next message runs in the
    /// agent's asking mode and its ask reaches the owner.
    #[tokio::test]
    async fn the_employees_mode_is_the_agents_own_from_the_next_message() {
        let r = remote("coder").await;
        let channels: AskChannels = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let mut full = request("check the repo", "chat-1", &remote_model());
        full.permission_mode = Some(types::permissions::Mode::FullAccess);
        full.ask_channels = Some(channels.clone());

        let events = collect(r.provider.stream(&full).await.unwrap()).await;
        assert_eq!(
            kinds(&events),
            vec![StreamEventType::ToolCall, StreamEventType::ToolResult, StreamEventType::Text, StreamEventType::Done],
            "the call ran and nothing asked"
        );
        assert!(channels.lock().await.is_empty(), "no permission request reached the owner");

        let mut ask = request("and again", "chat-1", &remote_model());
        ask.permission_mode = Some(types::permissions::Mode::Ask);
        ask.ask_channels = Some(channels.clone());
        let mut rx = r.provider.stream(&ask).await.unwrap();
        let (_, card) = until_ask(&mut rx).await;
        assert_eq!(card.widgets.as_ref().unwrap()[0]["options"], json!(["Allow once", "Deny"]));
        channels.lock().await.remove("call_1").expect("registered on the run").send("this_once".to_owned()).unwrap();
        let rest = collect(rx).await;
        assert_eq!(rest.last().unwrap().event_type, StreamEventType::Done);

        let folder = r.bot.agents()[0].acp.workdir.to_string_lossy().into_owned();
        assert_eq!(
            told(&r.told),
            vec![
                json!({ "new": folder }),
                json!({ "mode": "bypassPermissions" }),
                json!({ "prompt": [{ "type": "text", "text": "check the repo" }] }),
                json!({ "mode": "default" }),
                json!({ "prompt": [{ "type": "text", "text": "and again" }] }),
                json!({ "answer": { "outcome": "selected", "optionId": "allow" } }),
            ],
            "Full Access, then Ask, each the agent's own mode before its message"
        );
    }

    /// A question nothing on the run can answer is never left waiting: it
    /// is declined, and the turn goes on to its end.
    #[tokio::test]
    async fn an_ask_nothing_can_answer_is_declined() {
        let r = remote("ask").await;
        let mut req = request("clean the build", "chat-1", &remote_model());
        req.permission_mode = Some(types::permissions::Mode::Ask);
        let events = collect(r.provider.stream(&req).await.unwrap()).await;
        assert_eq!(kinds(&events), vec![StreamEventType::Text, StreamEventType::Done], "no card nobody could answer");
        assert_eq!(events[0].text, "answered: Deny");
        let answers: Vec<Value> = told(&r.told).into_iter().filter_map(|t| t.get("answer").cloned()).collect();
        assert_eq!(answers, vec![json!({ "outcome": "selected", "optionId": "deny" })]);
    }

    /// Stopped while the agent waits on the owner: the host answers its
    /// question as cancelled (ACP's `session/cancel`), once, and the card
    /// leaves the run, so the agent's turn ends and the next message is not
    /// stuck behind it.
    #[tokio::test]
    async fn a_stop_cancels_the_question_the_agent_waits_on() {
        let r = remote("ask").await;
        let channels: AskChannels = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let token = CancellationToken::new();
        let mut req = request("clean the build", "chat-1", &remote_model());
        req.ask_channels = Some(channels.clone());
        req.cancel_token = Some(token.clone());

        let mut rx = r.provider.stream(&req).await.unwrap();
        let (_, ask) = until_ask(&mut rx).await;
        assert_eq!(ask.error.as_deref(), Some("req-9"));
        token.cancel();
        let rest = collect(rx).await;
        assert_eq!(rest.last().unwrap().event_type, StreamEventType::Done);
        assert!(channels.lock().await.is_empty(), "the card left the run");
        // The host answers the question as it passes the cancel on, each on
        // its own way to the agent's process: the agent may end its turn,
        // and the stream end, before it has read the answer.
        let answered = || -> Vec<Value> { told(&r.told).into_iter().filter_map(|t| t.get("answer").cloned()).collect() };
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while answered().is_empty() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(answered(), vec![json!({ "outcome": "cancelled" })], "answered once, as cancelled");
    }

    /// A stop cancels the agent's turn, waits for its end, and ends the
    /// stream the way the CLI provider does.
    #[tokio::test]
    async fn cancel_reaches_the_runtime_and_ends_the_turn() {
        let r = remote("hang").await;
        let token = CancellationToken::new();
        let mut req = request("do something long", "chat-1", &remote_model());
        req.cancel_token = Some(token.clone());

        let mut rx = r.provider.stream(&req).await.unwrap();
        let first = tokio::time::timeout(Duration::from_secs(60), rx.recv()).await.unwrap().unwrap();
        assert_eq!(first.text, "Working");
        token.cancel();
        let rest = collect(rx).await;
        assert_eq!(kinds(&rest), vec![StreamEventType::Error, StreamEventType::Done]);
        assert_eq!(rest[0].error.as_deref(), Some("Cancelled"));
        assert_eq!(told(&r.told).iter().filter(|t| t.get("cancel").is_some()).count(), 1);
    }

    /// What a session's record says it is doing: the title, the request
    /// its latest turn works on (never an earlier one's), that turn's calls
    /// where each is now, and its latest words, bounded.
    #[test]
    fn a_sessions_work_is_read_from_its_record() {
        let user = |t: &str| json!({ "sessionUpdate": "user_message_chunk", "content": { "type": "text", "text": t } });
        let agent = |t: &str| json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": t } });
        let call = |id: &str, title: &str, status: &str| json!({ "sessionUpdate": "tool_call", "toolCallId": id, "title": title, "kind": "execute", "status": status });
        let done = |id: &str| json!({ "sessionUpdate": "tool_call_update", "toolCallId": id, "status": "completed" });
        let mut record = vec![
            json!({ "sessionUpdate": "session_info_update", "title": "Fix the login bug" }),
            user("earlier ask"),
            agent("earlier answer"),
            call("old", "old call", "in_progress"),
            user("find why sign-in "),
            user("fails"),
            agent("Looking."),
            call("c1", "grep auth", "in_progress"),
            done("c1"),
        ];
        for i in 2..=7 {
            record.push(call(&format!("c{i}"), &format!("step {i}"), "completed"));
        }
        record.push(call("c8", "cargo test", "in_progress"));
        record.push(agent(" Running the tests now."));
        let running = TurnUpdate {
            agent: "claude-code".into(),
            session_id: "s-1".into(),
            turn_id: "t-2".into(),
            state: TurnState::Running,
            started_at: "2026-09-28T16:00:00Z".into(),
            by: None,
            stop_reason: None,
            error: None,
            usage: None,
        };
        let work = summarise(&record, Some(&running));
        assert_eq!(work.title.as_deref(), Some("Fix the login bug"));
        assert_eq!(work.prompt.as_deref(), Some("find why sign-in fails"), "the latest request, whole, never an earlier one");
        assert_eq!(work.earlier_calls, 3, "eight calls this turn, the newest five carried");
        assert_eq!(work.calls.len(), WORK_CALLS);
        assert_eq!(work.calls.last().unwrap(), &("cargo test".to_owned(), "running".to_owned()));
        assert!(!work.calls.iter().any(|(l, _)| l == "old call"), "an earlier turn's call is not this one's: {:?}", work.calls);
        assert_eq!(work.said.as_deref(), Some("Looking. Running the tests now."));
        assert_eq!(work.running_since.as_deref(), Some("2026-09-28T16:00:00Z"));

        let long = "x".repeat(WORK_CHARS + 50);
        let work = summarise(&[user(&long), agent(&long)], None);
        assert!(work.prompt.as_deref().unwrap().ends_with(" …"), "a long request says where it is cut");
        assert!(work.said.as_deref().unwrap().starts_with("… "), "the latest words keep their end");
        assert_eq!(work.running_since, None, "no turn runs");
        assert_eq!(summarise(&[], None), SessionWork::default());
    }

    /// A linked agent mid-prompt, asked from outside its turn: its host says
    /// it is working on a prompt in that session, an agent it does not list
    /// is `None`, and the session's record gives the request it works on and
    /// what it said, while the turn goes on untouched.
    #[tokio::test]
    async fn the_host_says_what_its_agent_is_doing_and_its_session_is_read() {
        let r = remote("hang").await;
        let token = CancellationToken::new();
        let mut req = request("refactor the billing module", "chat-1", &remote_model());
        req.cancel_token = Some(token.clone());
        let mut rx = r.provider.stream(&req).await.unwrap();
        let first = tokio::time::timeout(Duration::from_secs(60), rx.recv()).await.unwrap().unwrap();
        assert_eq!(first.text, "Working");

        assert!(!r.provider.hosted_here(BOT), "another computer's bot");
        let status = r.provider.status(BOT, &["claude-code".to_owned(), "nobody".to_owned()]).await.unwrap();
        let codex = status[0].as_ref().expect("the host lists its agent");
        assert!(codex.busy && codex.why.contains(&Working::Prompt), "{codex:?}");
        let session = codex.sessions.iter().find(|s| s.session_id == "s-1").expect("the session it works in");
        assert!(session.busy && session.why.contains(&Working::Prompt), "{session:?}");
        assert!(status[1].is_none(), "an agent the host does not list");

        let work = r.provider.session_work(BOT, "claude-code", "s-1").await.unwrap();
        assert_eq!(work.prompt.as_deref(), Some("refactor the billing module"));
        assert_eq!(work.said.as_deref(), Some("Working"));
        assert!(work.running_since.is_some(), "{work:?}");

        let told = told(&r.told);
        assert_eq!(told.iter().filter(|t| t.get("prompt").is_some()).count(), 1, "reading sent nothing: {told:?}");
        assert!(told.iter().all(|t| t.get("cancel").is_none()), "reading cancelled nothing: {told:?}");
        token.cancel();
        let rest = collect(rx).await;
        assert_eq!(rest[0].error.as_deref(), Some("Cancelled"));
    }

    /// Never two prompts in flight on one session: a message for a session
    /// whose turn runs waits, the agent is told nothing of it, and stopping
    /// it withdraws it without cancelling the turn it waited behind.
    #[tokio::test]
    async fn a_waiting_message_never_cancels_the_turn_it_waits_behind() {
        let r = remote("hang").await;
        let first = CancellationToken::new();
        let mut req = request("do something long", "chat-1", &remote_model());
        req.cancel_token = Some(first.clone());
        let mut running = r.provider.stream(&req).await.unwrap();
        let said = tokio::time::timeout(Duration::from_secs(60), running.recv()).await.unwrap().unwrap();
        assert_eq!(said.text, "Working");

        // A second Nebo conversation bound to the same session.
        r.store.create_chat("chat-2", "Second").unwrap();
        r.store.set_chat_linked_session("chat-2", "claude-code", "s-1").unwrap();
        let second = CancellationToken::new();
        let mut req = request("and also this", "chat-2", &remote_model());
        req.cancel_token = Some(second.clone());
        let waiting = r.provider.stream(&req).await.unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        let prompts = |told: &[Value]| told.iter().filter(|t| t.get("prompt").is_some()).count();
        assert_eq!(prompts(&told(&r.told)), 1, "the second message waits: {:?}", told(&r.told));

        second.cancel();
        let withdrawn = collect(waiting).await;
        assert_eq!(withdrawn[0].error.as_deref(), Some("Cancelled"));
        tokio::time::sleep(Duration::from_millis(300)).await;
        let told_now = told(&r.told);
        assert!(told_now.iter().all(|t| t.get("cancel").is_none()), "the running turn was not cancelled: {told_now:?}");
        assert_eq!(prompts(&told_now), 1, "the withdrawn message never reached the agent");
        let status = r.provider.status(BOT, &["claude-code".to_owned()]).await.unwrap();
        assert!(status[0].as_ref().unwrap().busy, "the first turn goes on");

        first.cancel();
        let rest = collect(running).await;
        assert_eq!(rest[0].error.as_deref(), Some("Cancelled"));
        assert_eq!(told(&r.told).iter().filter(|t| t.get("cancel").is_some()).count(), 1);
    }

    /// The owner's "Stop… now do X" still has Claude's context: the agent
    /// acknowledged the cancel (its own record of the session is intact),
    /// so the session is kept, and the next message resumes it
    /// (`session/load`) instead of starting over (`session/new`).
    #[tokio::test]
    async fn an_acknowledged_cancel_keeps_the_session() {
        let r = remote("hang").await;
        let token = CancellationToken::new();
        let mut req = request("do something long", "chat-1", &remote_model());
        req.cancel_token = Some(token.clone());

        let mut rx = r.provider.stream(&req).await.unwrap();
        tokio::time::timeout(Duration::from_secs(60), rx.recv()).await.unwrap().unwrap();
        token.cancel();
        let rest = collect(rx).await;
        assert_eq!(rest[0].error.as_deref(), Some("Cancelled"));

        let chat = r.store.get_chat("chat-1").unwrap().unwrap();
        assert!(chat.linked_chat_id.is_some_and(|s| !s.is_empty()), "the session is kept after an acknowledged cancel");

        let mut again = r.provider.stream(&request("now do X", "chat-1", &remote_model())).await.unwrap();
        tokio::time::timeout(Duration::from_secs(60), again.recv()).await.unwrap().unwrap();
        let news = told(&r.told).iter().filter(|t| t.get("new").is_some()).count();
        assert_eq!(news, 1, "resumed the same session, never session/new again");
    }

    /// A cancel that never gets an answer — the agent deaf to it, Nebo's own
    /// [`CANCEL_TIMEOUT`] running out — is the wedge case: nothing vouches
    /// for the session's state, so it is forgotten and the next message
    /// opens a fresh one instead of resuming one that might never answer
    /// again.
    #[tokio::test]
    async fn an_unacknowledged_cancel_forgets_the_session() {
        let r = remote("deaf").await;
        let token = CancellationToken::new();
        let mut req = request("do something long", "chat-1", &remote_model());
        req.cancel_token = Some(token.clone());

        let mut rx = r.provider.stream(&req).await.unwrap();
        tokio::time::timeout(Duration::from_secs(60), rx.recv()).await.unwrap().unwrap();
        token.cancel();
        let rest = collect(rx).await;
        assert_eq!(rest[0].error.as_deref(), Some("Cancelled"));

        let chat = r.store.get_chat("chat-1").unwrap().unwrap();
        assert!(chat.linked_chat_id.is_none_or(|s| s.is_empty()), "the session is forgotten after an unacknowledged cancel");

        let mut again = r.provider.stream(&request("and again", "chat-1", &remote_model())).await.unwrap();
        tokio::time::timeout(Duration::from_secs(60), again.recv()).await.unwrap().unwrap();
        let news = told(&r.told).iter().filter(|t| t.get("new").is_some()).count();
        assert_eq!(news, 2, "session/new again, not a resume of the forgotten one");
    }

    /// Generous enough for several [`HEARTBEAT`]s (test-shrunk) to pass in
    /// real time.
    async fn collect_patiently(mut rx: EventReceiver) -> Vec<StreamEvent> {
        let cap = HEARTBEAT * 20 + Duration::from_secs(5);
        let mut events = Vec::new();
        while let Some(e) = tokio::time::timeout(cap, rx.recv()).await.expect("the turn went on") {
            events.push(e);
        }
        events
    }

    /// A turn that works in silence — a build, a test run, a long call — is
    /// never cut: the host says a call runs, so however long it is quiet,
    /// nothing ends it but the owner's own stop, and the session is kept.
    #[tokio::test]
    async fn a_quiet_turn_that_works_is_never_cut() {
        let r = remote("busy").await;
        let token = CancellationToken::new();
        let mut req = briefed(request("build it", "chat-1", &remote_model()));
        req.cancel_token = Some(token.clone());

        let mut rx = r.provider.stream(&req).await.unwrap();
        let first = tokio::time::timeout(Duration::from_secs(60), rx.recv()).await.unwrap().unwrap();
        assert_eq!(first.event_type, StreamEventType::ToolCall, "the running call is shown");

        let quiet = tokio::time::timeout(STALL_WINDOW * 2, rx.recv()).await;
        assert!(quiet.is_err(), "a quiet turn that works was ended on its own");
        let told_now = told(&r.told);
        assert_eq!(news(&told_now), 1, "never a fresh session: {told_now:?}");
        assert!(told_now.iter().all(|t| t.get("cancel").is_none()), "never cancelled: {told_now:?}");

        token.cancel();
        let rest = collect(rx).await;
        assert_eq!(rest[0].error.as_deref(), Some("Cancelled"), "only the owner's stop ends it");
    }

    /// The live incident: the agent's stream stopped mid-call (the call still
    /// pending, then nothing) and its process sat idle. Its host says the
    /// prompt is open and nothing works; held for the stall window, the turn
    /// is not left hanging: the stuck session is cancelled and forgotten,
    /// the owner reads one plain line, and the conversation continues in a
    /// fresh session whose first prompt carries the handoff (the briefing,
    /// Nebo's summary, the latest exchanges word for word) and the owner's
    /// message, which it answers.
    #[tokio::test]
    async fn a_stalled_turn_continues_in_a_fresh_session_with_a_handoff() {
        let r = remote("stall").await;
        let events = collect(r.provider.stream(&briefed(request("do it", "chat-1", &remote_model()))).await.unwrap()).await;
        assert_eq!(events.last().unwrap().event_type, StreamEventType::Done, "{:?}", events.iter().map(|e| (&e.text, &e.error)).collect::<Vec<_>>());
        assert_eq!(texts(&events), format!("{}\n\nFresh here.", NOTICE));

        let told_now = told(&r.told);
        assert_eq!(news(&told_now), 2, "{told_now:?}");
        assert!(told_now.iter().any(|t| t.get("cancel").is_some()), "the stuck turn was cancelled: {told_now:?}");
        let prompts = prompt_texts(&told_now);
        assert_eq!(prompts[0], ["BRIEFING", "do it"]);
        let fresh = prompts.last().unwrap();
        assert_eq!(fresh[1], "do it", "the owner's message, not retyped");
        assert_handoff(&fresh[0]);
        let chat = r.store.get_chat("chat-1").unwrap().unwrap();
        assert_eq!(chat.linked_chat_id.as_deref(), Some("s-2"), "the fresh session is the conversation's now");
    }

    /// The agent acknowledges a cancel (its adapter forces it) while its
    /// session stays wedged: the session is kept, as for any acknowledged
    /// cancel, and the next message, resumed there and stalled again, still
    /// continues in a fresh session and is answered.
    #[tokio::test]
    async fn an_acknowledged_cancel_on_a_wedged_session_still_gets_a_fresh_one() {
        let r = remote("stall").await;
        let token = CancellationToken::new();
        let mut req = briefed(request("do it", "chat-1", &remote_model()));
        req.cancel_token = Some(token.clone());
        let rx = r.provider.stream(&req).await.unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while !told(&r.told).iter().any(|t| t.get("prompt").is_some()) {
            assert!(std::time::Instant::now() < deadline, "the prompt never arrived");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        tokio::time::sleep(HEARTBEAT * 2).await;
        token.cancel();
        let rest = collect(rx).await;
        let error = rest.iter().find(|e| e.event_type == StreamEventType::Error).and_then(|e| e.error.as_deref());
        assert_eq!(error, Some("Cancelled"));
        assert_eq!(r.store.get_chat("chat-1").unwrap().unwrap().linked_chat_id.as_deref(), Some("s-1"), "an acknowledged cancel keeps the session");

        let next = collect(r.provider.stream(&briefed(request("now do X", "chat-1", &remote_model()))).await.unwrap()).await;
        assert_eq!(texts(&next), format!("{}\n\nFresh here.", NOTICE), "{:?}", next.iter().map(|e| (&e.text, &e.error)).collect::<Vec<_>>());
        let prompts = prompt_texts(&told(&r.told));
        assert_eq!(prompts.last().unwrap()[1], "now do X");
        assert_eq!(r.store.get_chat("chat-1").unwrap().unwrap().linked_chat_id.as_deref(), Some("s-2"));
    }

    /// A fresh session's first prompt starts with the briefing. A healthy,
    /// short session then carries on from message to message: resumed, it is
    /// sent only the owner's message, never briefed again, and nothing
    /// rotates it. Its activity is kept on the chat.
    #[tokio::test]
    async fn a_fresh_session_is_briefed_once_and_a_healthy_one_carries_on() {
        let r = remote("rotate").await;
        let first = collect(r.provider.stream(&briefed(request("hi", "chat-1", &remote_model()))).await.unwrap()).await;
        assert_eq!(texts(&first), "In s-1.");
        let second = collect(r.provider.stream(&briefed(request("and again", "chat-1", &remote_model()))).await.unwrap()).await;
        assert_eq!(texts(&second), "In s-1.", "the same session, and no notice");
        let told_now = told(&r.told);
        assert_eq!(prompt_texts(&told_now), vec![vec!["BRIEFING", "hi"], vec!["and again"]]);
        assert_eq!(news(&told_now), 1);
        let chat = r.store.get_chat("chat-1").unwrap().unwrap();
        assert_eq!((chat.linked_used_tokens, chat.linked_window_tokens), (Some(1_000), Some(200_000)));
        assert!(chat.linked_turn_at.is_some());
    }

    /// A session that has used most of its context window, as its agent
    /// reported, continues fresh at the next message, with the handoff.
    #[tokio::test]
    async fn a_session_that_got_long_continues_fresh_with_a_handoff() {
        let r = remote("rotate").await;
        collect(r.provider.stream(&briefed(request("a long one", "chat-1", &remote_model()))).await.unwrap()).await;
        let next = collect(r.provider.stream(&briefed(request("next", "chat-1", &remote_model()))).await.unwrap()).await;
        assert_eq!(texts(&next), format!("{}\n\nIn s-2.", NOTICE));
        let prompts = prompt_texts(&told(&r.told));
        assert_eq!(prompts.len(), 2);
        assert_eq!(prompts[1][1], "next");
        assert_handoff(&prompts[1][0]);
        let chat = r.store.get_chat("chat-1").unwrap().unwrap();
        assert_eq!((chat.linked_chat_id.as_deref(), chat.linked_used_tokens), (Some("s-2"), Some(1_000)), "the fresh session's own activity");
    }

    /// A session with no turn for a long time continues fresh at the next
    /// message, with the handoff.
    #[tokio::test]
    async fn a_session_idle_for_hours_continues_fresh_with_a_handoff() {
        let r = remote("rotate").await;
        collect(r.provider.stream(&briefed(request("hi", "chat-1", &remote_model()))).await.unwrap()).await;
        r.store.set_chat_linked_activity("chat-1", now_secs() - 9 * 3600, None).unwrap();
        let next = collect(r.provider.stream(&briefed(request("morning", "chat-1", &remote_model()))).await.unwrap()).await;
        assert_eq!(texts(&next), format!("{}\n\nIn s-2.", NOTICE));
        let prompts = prompt_texts(&told(&r.told));
        assert_handoff(&prompts[1][0]);
        assert_eq!(prompts[1][1], "morning");
    }

    /// `/clear` is a true reset: the conversation's messages and its session
    /// are gone, and the next message starts a fresh session with its
    /// briefing and no handoff, and no notice.
    #[tokio::test]
    async fn a_cleared_conversation_starts_fresh_with_no_handoff() {
        let r = remote("rotate").await;
        collect(r.provider.stream(&briefed(request("hi", "chat-1", &remote_model()))).await.unwrap()).await;
        // What `/clear` does to a thread.
        r.store.delete_chat_messages_by_chat_id("chat-1").unwrap();
        r.store.set_chat_linked_session("chat-1", "", "").unwrap();
        let mut req = briefed(request("start over", "chat-1", &remote_model()));
        req.messages.drain(..2);
        let next = collect(r.provider.stream(&req).await.unwrap()).await;
        assert_eq!(texts(&next), "In s-2.");
        assert_eq!(prompt_texts(&told(&r.told))[1], ["BRIEFING", "start over"]);
    }

    /// The owner's `/compact` compacts the agent's own session where the
    /// agent offers the command: sent to it as his message, in the same
    /// session, and its answer streams back.
    #[tokio::test]
    async fn compact_runs_in_the_agents_own_session_where_it_offers_it() {
        let r = remote("rotate").await;
        collect(r.provider.stream(&briefed(request("hi", "chat-1", &remote_model()))).await.unwrap()).await;
        let compacted = collect(r.provider.stream(&briefed(request("/compact keep the numbers", "chat-1", &remote_model()))).await.unwrap()).await;
        assert_eq!(texts(&compacted), "Compacted.");
        assert_eq!(compacted.last().unwrap().event_type, StreamEventType::Done);
        let told_now = told(&r.told);
        assert_eq!(prompt_texts(&told_now).last().unwrap(), &["/compact keep the numbers"]);
        assert_eq!(news(&told_now), 1, "the same session");
    }

    /// An agent that offers no compact command: the owner reads plainly that
    /// it can't, and the agent is sent nothing.
    #[tokio::test]
    async fn compact_is_refused_plainly_where_the_agent_offers_none() {
        let r = remote("turn").await;
        collect(r.provider.stream(&request("hi", "chat-1", &remote_model())).await.unwrap()).await;
        let refused = collect(r.provider.stream(&request("/compact", "chat-1", &remote_model())).await.unwrap()).await;
        assert_eq!(kinds(&refused), vec![StreamEventType::Error, StreamEventType::Done]);
        assert_eq!(refused[0].error.as_deref(), Some("Claude Code can't compact its conversation from here."));
        assert!(prompt_texts(&told(&r.told)).iter().all(|p| p.last().map(String::as_str) != Some("/compact")));
        assert!(is_command("/Compact now", COMPACT) && !is_command("/compaction", COMPACT) && !is_command("compact it", COMPACT));
    }

    /// A turn is stalled only when its host says the prompt is open and
    /// nothing works in it, with nothing new from the agent, for the whole
    /// window: a running call, a question waiting or the agent's processes
    /// working never count, something new starts the window over, and no
    /// word from the host is no evidence.
    #[test]
    fn a_turn_is_stalled_only_when_nothing_works_and_nothing_new_comes_for_the_window() {
        use std::time::Instant;
        let status = |why: Vec<Working>, agent: Vec<Working>, last: &str| AgentStatus {
            agent: "claude-code".into(),
            state: Life::Running,
            busy: true,
            why: agent,
            idle_since: None,
            sessions: vec![SessionStatus { session_id: "s".into(), state: Life::Running, busy: true, why, last_update: Some(last.into()) }],
        };
        let (w, t0, minute) = (Duration::from_secs(600), Instant::now(), Duration::from_secs(60));
        let quiet = status(vec![Working::Prompt], vec![Working::Prompt], "10:00");

        let mut stall = Stall::default();
        assert!(!stall.look(Some(&quiet), "s", t0, w));
        assert!(!stall.look(Some(&quiet), "s", t0 + w - Duration::from_secs(1), w));
        assert!(stall.look(Some(&quiet), "s", t0 + w, w));

        for (why, agent) in [
            (vec![Working::Prompt, Working::Tool], vec![Working::Prompt, Working::Tool]),
            (vec![Working::Prompt, Working::Permission], vec![Working::Prompt, Working::Permission]),
            (vec![Working::Prompt], vec![Working::Prompt, Working::Processes]),
        ] {
            let mut stall = Stall::default();
            for m in 0..=120 {
                assert!(!stall.look(Some(&status(why.clone(), agent.clone(), "10:00")), "s", t0 + minute * m, w), "{why:?} {agent:?}");
            }
        }

        // Something new from the agent starts the window over.
        let mut stall = Stall::default();
        assert!(!stall.look(Some(&quiet), "s", t0, w));
        let newer = status(vec![Working::Prompt], vec![Working::Prompt], "10:05");
        assert!(!stall.look(Some(&newer), "s", t0 + w, w));
        assert!(stall.look(Some(&newer), "s", t0 + w * 2, w));

        // So does a frame Nebo read; and a host that says nothing, a paused
        // agent or another session is no evidence.
        let mut stall = Stall::default();
        stall.look(Some(&quiet), "s", t0, w);
        stall.heard();
        assert!(!stall.look(Some(&quiet), "s", t0 + w, w));
        assert!(!stall.look(None, "s", t0 + w * 2, w));
        let paused = AgentStatus { state: Life::Paused, ..quiet.clone() };
        assert!(!stall.look(Some(&paused), "s", t0 + w * 3, w));
        assert!(!stall.look(Some(&quiet), "other", t0 + w * 4, w));
        assert!(!stall.look(Some(&quiet), "s", t0 + w * 5 - Duration::from_secs(1), w));
    }

    /// When a recorded session continues fresh at the next message: idle for
    /// hours, or most of its context window used; never otherwise.
    #[test]
    fn a_session_continues_fresh_when_it_sat_idle_or_got_long() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = store_in(dir.path());
        store.set_chat_linked_session("chat-1", "claude-code", "s-1").unwrap();
        let now = 1_000_000;
        let due = |at: i64, context: Option<(i64, i64)>| {
            store.set_chat_linked_session("chat-1", "claude-code", "").unwrap();
            store.set_chat_linked_session("chat-1", "claude-code", "s-1").unwrap();
            store.set_chat_linked_activity("chat-1", at, context).unwrap();
            rotation_due(&store.get_chat("chat-1").unwrap().unwrap(), now)
        };
        assert_eq!(due(now - 60, Some((10_000, 200_000))), None);
        assert_eq!(due(now - 60, None), None, "a session whose agent reports no usage");
        assert_eq!(due(now - 60, Some((119_999, 200_000))), None);
        assert_eq!(due(now - 60, Some((120_000, 200_000))), Some(Rotation::Long));
        assert_eq!(due(now - 8 * 3600 + 1, Some((1, 200_000))), None);
        assert_eq!(due(now - 8 * 3600, Some((1, 200_000))), Some(Rotation::Idle));
        assert_eq!(rotation_due(&store.get_chat("chat-1").unwrap().unwrap(), now - 8 * 3600), None);
    }

    /// The Mac-mini incident, precisely: the agent's own process dies
    /// mid-turn — not a network blip, the process itself is gone. In this
    /// host, that takes the connection down with it (nothing left for it to
    /// relay), so this exercises the same reconnect-exhaustion path as
    /// [`a_connection_that_never_comes_back_ends_the_turn_cleanly`]: no
    /// process to reconnect to, the window runs out, and the turn ends
    /// cleanly instead of sitting on "Thinking…" forever. A host that keeps
    /// a connection open past one of several agents dying is exactly what
    /// [`Driver::liveness`]'s own `host/agents` check is for — not
    /// reachable through this single-agent test fixture, but reached the
    /// same way once the connection itself does drop.
    #[tokio::test]
    async fn an_agent_whose_process_died_ends_the_turn_cleanly() {
        let r = remote("dies").await;
        let req = request("do something long", "chat-1", &remote_model());

        let mut rx = r.provider.stream(&req).await.unwrap();
        let first = tokio::time::timeout(Duration::from_secs(60), rx.recv()).await.unwrap().unwrap();
        assert_eq!(first.text, "Working");

        let rest = collect_patiently(rx).await;
        assert_eq!(kinds(&rest), vec![StreamEventType::Error, StreamEventType::Done]);
        assert_eq!(rest[0].error.as_deref(), Some("Could not connect to Danny. Try again."));
    }

    /// The connection itself is gone and no reconnect attempt ever
    /// succeeds (the bot unreachable, not just one socket): the turn ends
    /// cleanly once the reconnect window is exhausted, the same plain
    /// "could not connect" copy as never having reached it at all — never a
    /// hang.
    #[tokio::test]
    async fn a_connection_that_never_comes_back_ends_the_turn_cleanly() {
        let r = remote("hang").await;
        let req = request("do something long", "chat-1", &remote_model());

        let mut rx = r.provider.stream(&req).await.unwrap();
        let first = tokio::time::timeout(Duration::from_secs(60), rx.recv()).await.unwrap().unwrap();
        assert_eq!(first.text, "Working");

        r.hub.lock().unwrap().shut_down();

        let rest = collect_patiently(rx).await;
        assert_eq!(kinds(&rest), vec![StreamEventType::Error, StreamEventType::Done]);
        assert_eq!(rest[0].error.as_deref(), Some("Could not connect to Danny. Try again."));
    }

    /// Nothing answers at the linked bot: the plain copy, no retry.
    #[tokio::test]
    async fn an_unreachable_link_answers_with_the_plain_copy() {
        let root = tempfile::tempdir().unwrap();
        let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", closed.local_addr().unwrap());
        drop(closed);
        let (store, _) = store_in(root.path());
        let relay = Relay::Hub {
            api_url: url,
            token: Arc::new(|| Some("bot-jwt".to_owned())),
        };
        let p = LinkedProvider::new(relay, store, Some(local_host(root.path())), "Nebo on test");
        assert!(!p.retryable());

        let events = collect(p.stream(&request("hello", "chat-1", &remote_model())).await.unwrap()).await;
        assert_eq!(kinds(&events), vec![StreamEventType::Error, StreamEventType::Done]);
        assert_eq!(events[0].error.as_deref(), Some("Could not connect to Danny. Try again."));
    }

    /// The connection drops while the agent waits on the owner: Nebo reaches
    /// the bot again, loads the session, and the turn goes on. The owner's
    /// answer, given while Nebo was away, is sent to the question the host
    /// asks again; nothing Nebo already showed is shown twice.
    #[tokio::test]
    async fn a_turn_survives_a_reconnect_while_it_waits_on_the_owner() {
        let r = remote("git").await;
        let channels: AskChannels = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let mut req = request("check the repo", "chat-1", &remote_model());
        req.ask_channels = Some(channels.clone());

        let mut rx = r.provider.stream(&req).await.unwrap();
        let (before, ask) = until_ask(&mut rx).await;
        assert_eq!(kinds(&before), vec![StreamEventType::Text, StreamEventType::ToolCall]);
        assert_eq!(ask.error.as_deref(), Some("call_1"));

        r.hub.lock().unwrap().drop_sockets();
        channels.lock().await.remove("call_1").expect("registered on the run").send("Allow once".to_owned()).unwrap();

        let rest = collect(rx).await;
        assert_eq!(
            kinds(&rest),
            vec![StreamEventType::ToolResult, StreamEventType::Text, StreamEventType::Usage, StreamEventType::Done],
            "{:?}",
            rest.iter().map(|e| (&e.text, &e.error)).collect::<Vec<_>>()
        );
        assert_eq!(rest[1].text, "Ran it.");
        let answers: Vec<Value> = told(&r.told).into_iter().filter_map(|t| t.get("answer").cloned()).collect();
        assert_eq!(answers, vec![json!({ "outcome": "selected", "optionId": "allow" })], "answered once");
        assert_eq!(r.hub.lock().unwrap().bearers.len(), 3, "the pairing, then one reconnect");
    }

    /// The bot's nebo-link daemon runs on this computer, and NeboAI's relay
    /// fails (502) the whole time: the turn opens on this computer, its
    /// connection drops while the agent waits on the owner, Nebo reaches the
    /// bot again on this computer at once, and the turn ends as it would have.
    /// The relay is asked for nothing but the pairing code.
    #[tokio::test]
    async fn a_turn_with_a_bot_on_this_computer_never_waits_on_a_failing_relay() {
        let r = remote("git").await;
        let bot = r.bot.oal().unwrap();
        let serving = oal_host::lan::serve(bot, "127.0.0.1:0".parse().unwrap(), &r._root.path().join("bot-tls"), oal_host::lan::Reach::Machine)
            .await
            .unwrap();
        // The daemon's listener, behind a pipe the test can cut.
        let pipe = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let direct = link_core::machine::Direct { addr: pipe.local_addr().unwrap(), fingerprint: serving.fingerprint };
        link_core::machine::record_direct(&r._root.path().join("nebo").join("nebo-link").join(BOT), &direct, true).unwrap();
        let piped: Arc<Mutex<Vec<tokio::task::AbortHandle>>> = Arc::default();
        let pipes = piped.clone();
        tokio::spawn(async move {
            loop {
                let (mut client, _) = pipe.accept().await.unwrap();
                let task = tokio::spawn(async move {
                    let mut host = TcpStream::connect(serving.addr).await.unwrap();
                    let _ = tokio::io::copy_bidirectional(&mut client, &mut host).await;
                });
                pipes.lock().unwrap().push(task.abort_handle());
            }
        });
        r.hub.lock().unwrap().failing = true;

        let channels: AskChannels = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let mut req = request("check the repo", "chat-1", &remote_model());
        req.ask_channels = Some(channels.clone());
        let mut rx = r.provider.stream(&req).await.unwrap();
        let (before, ask) = until_ask(&mut rx).await;
        assert_eq!(kinds(&before), vec![StreamEventType::Text, StreamEventType::ToolCall]);
        assert_eq!(ask.error.as_deref(), Some("call_1"));

        for pipe in piped.lock().unwrap().drain(..) {
            pipe.abort();
        }
        channels.lock().await.remove("call_1").expect("registered on the run").send("Allow once".to_owned()).unwrap();

        let rest = tokio::time::timeout(Duration::from_secs(10), collect(rx)).await.expect("the turn never waited on the relay");
        assert_eq!(
            kinds(&rest),
            vec![StreamEventType::ToolResult, StreamEventType::Text, StreamEventType::Usage, StreamEventType::Done],
            "{:?}",
            rest.iter().map(|e| (&e.text, &e.error)).collect::<Vec<_>>()
        );
        assert_eq!(rest[1].text, "Ran it.");
        assert_eq!(r.hub.lock().unwrap().bearers, vec!["bearer bot-jwt"], "the pairing code, and no socket");
    }

    /// The first answer wins: answered on the phone (another client of the
    /// bot's host), the question leaves Nebo's run, and the turn goes on.
    #[tokio::test]
    async fn an_answer_on_the_phone_takes_nebos_card_back() {
        let r = remote("git").await;
        let channels: AskChannels = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let mut req = request("check the repo", "chat-1", &remote_model());
        req.ask_channels = Some(channels.clone());

        let mut rx = r.provider.stream(&req).await.unwrap();
        let (_, ask) = until_ask(&mut rx).await;
        assert_eq!(ask.error.as_deref(), Some("call_1"));
        let host = r.bot.oal().unwrap().host().clone();
        let pending = host.pending();
        assert_eq!(pending.len(), 1);
        let phone = DeviceRef {
            device_id: "d-phone".into(),
            name: "Alma's phone".into(),
        };
        host.answer(&pending[0].id, link_core::model::Outcome::Selected { option_id: "allow".into() }, Some(phone)).unwrap();

        let rest = collect(rx).await;
        assert_eq!(
            kinds(&rest),
            vec![StreamEventType::ToolResult, StreamEventType::Text, StreamEventType::Usage, StreamEventType::Done]
        );
        assert!(channels.lock().await.get("call_1").is_none(), "Nebo's card was taken back");
    }

    /// A bot that forgot Nebo (unpaired it) is paired with again by itself on
    /// the next turn.
    #[tokio::test]
    async fn a_bot_that_forgot_nebo_is_paired_again() {
        let r = remote("turn").await;
        let first = collect(r.provider.stream(&request("list the files", "chat-1", &remote_model())).await.unwrap()).await;
        assert_eq!(first.last().unwrap().event_type, StreamEventType::Done);
        let oal = r.bot.oal().unwrap();
        let nebo = oal.devices().remove(0);
        oal.unpair(&nebo.id).await.unwrap();

        let second = collect(r.provider.stream(&request("and now?", "chat-1", &remote_model())).await.unwrap()).await;
        assert_eq!(second.last().unwrap().event_type, StreamEventType::Done, "{:?}", second.iter().map(|e| &e.error).collect::<Vec<_>>());
        assert_eq!(oal.devices().len(), 1, "paired again");
        assert_ne!(oal.devices()[0].id, nebo.id);
    }

    // -- A coding agent on this computer ----------------------------------

    /// A coding agent hired on this computer runs in Nebo, in its own
    /// folder: a turn reaches it, its permission request is an ask on the
    /// run's ask channels with its own options in the owner's words, the
    /// answer goes back, and the turn completes in the employee's mode. No
    /// hub is reachable and no NeboAI token exists: none is needed.
    #[tokio::test]
    async fn a_coding_agent_on_this_computer_runs_in_nebo_with_no_hub() {
        let root = tempfile::tempdir().unwrap();
        let told_path = root.path().join("told.jsonl");
        let local = local_host(root.path());
        let hosted = local.host(nebo_runtimes::acp::Agent::ClaudeCode, fake_acp(&told_path, "git")).await.unwrap();
        assert_eq!((hosted.id.as_str(), hosted.label.as_str()), ("claude-code", "Claude Code"));
        let folder = root.path().join("home").join("NeboAI").join("claude-code").canonicalize().unwrap();
        assert_eq!(hosted.acp.workdir, folder, "its own folder under ~/NeboAI");
        let second = local.host(nebo_runtimes::acp::Agent::ClaudeCode, fake_acp(&told_path, "git")).await.unwrap();
        assert_eq!((second.id.as_str(), second.label.as_str()), ("claude-code-2", "Claude Code 2"));
        // nebo-link reads that Nebo hosts this computer's agents, and so
        // refuses to link or pair: one program hosts them.
        let daemon_home = root.path().join("nebo-link");
        let hosting = link_core::machine::hosting_app(&daemon_home).expect("recorded");
        assert_eq!(hosting.app, "Nebo");
        assert_eq!(hosting.agents, ["Claude Code", "Claude Code 2"]);

        let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let hub = format!("http://{}", closed.local_addr().unwrap());
        drop(closed);
        let (store, db_path) = store_in(root.path());
        let relay = Relay::Hub {
            api_url: hub,
            token: Arc::new(|| None),
        };
        let p = LinkedProvider::new(relay, store.clone(), Some(local.clone()), "Nebo on test");
        let channels: AskChannels = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let mut req = request("check the repo", "chat-1", &format!("{SELF}/{}", hosted.id));
        req.permission_mode = Some(types::permissions::Mode::Automatic);
        req.ask_channels = Some(channels.clone());

        let mut rx = p.stream(&req).await.unwrap();
        let (before, ask) = until_ask(&mut rx).await;
        assert_eq!(kinds(&before), vec![StreamEventType::Text, StreamEventType::ToolCall]);
        assert_eq!(before[0].text, "Checking.");
        assert_eq!(before[1].tool_call.as_ref().unwrap().name, "git status");
        assert_eq!(ask.error.as_deref(), Some("call_1"), "the question's id");
        assert_eq!(ask.text, "git status");
        assert_eq!(ask.widgets.as_ref().unwrap()[0]["options"], json!(["Allow once", "Deny"]));

        // The ask card's answer, through the ONE pathway.
        channels.lock().await.remove("call_1").expect("registered on the run").send("Allow once".to_owned()).unwrap();
        let rest = collect(rx).await;
        assert_eq!(
            kinds(&rest),
            vec![StreamEventType::ToolResult, StreamEventType::Text, StreamEventType::Usage, StreamEventType::Done]
        );
        assert_eq!(rest[0].text, "nothing to commit");
        assert_eq!(rest[1].text, "Ran it.");
        let usage = rest[2].usage.as_ref().unwrap();
        assert_eq!((usage.input_tokens, usage.output_tokens), (7, 3));

        assert_eq!(
            told(&told_path),
            vec![
                json!({ "new": folder.to_string_lossy() }),
                json!({ "mode": "acceptEdits" }),
                json!({ "prompt": [{ "type": "text", "text": "check the repo" }] }),
                json!({ "answer": { "outcome": "selected", "optionId": "allow" } }),
            ],
            "its session in its folder, the employee's mode, then only the newest message, then the owner's answer"
        );
        let chat = store.get_chat("chat-1").unwrap().unwrap();
        assert_eq!(chat.linked_chat_id.as_deref(), Some("s-1"), "the Nebo chat records the agent's session");
        assert_eq!(chat.linked_agent_id.as_deref(), Some("claude-code"), "and the agent it is");

        // A chat recorded before its agent was (the phone contract's
        // `claude-code~s-1`, migrated to `s-1`) goes on in the same session,
        // and records its agent.
        rusqlite::Connection::open(&db_path)
            .unwrap()
            .execute("UPDATE chats SET linked_agent_id = NULL WHERE id = 'chat-1'", [])
            .unwrap();
        let mut again = request("and now?", "chat-1", &format!("{SELF}/{}", hosted.id));
        again.ask_channels = Some(channels.clone());
        let mut rx = p.stream(&again).await.unwrap();
        let (_, ask) = until_ask(&mut rx).await;
        channels.lock().await.remove(ask.error.as_deref().unwrap()).unwrap().send("Deny".to_owned()).unwrap();
        let rest = collect(rx).await;
        assert_eq!(rest.last().unwrap().event_type, StreamEventType::Done);
        assert_eq!(told(&told_path).iter().filter(|t| t.get("new").is_some()).count(), 1, "the same session, no new one");
        assert_eq!(store.get_chat("chat-1").unwrap().unwrap().linked_agent_id.as_deref(), Some("claude-code"));

        // Fired: the agent is no longer hosted, and its folder stays.
        local.remove(&hosted.id).await.unwrap();
        assert_eq!(local.agents().iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["claude-code-2"]);
        assert_eq!(link_core::machine::hosting_app(&daemon_home).unwrap().agents, ["Claude Code 2"]);
        assert!(folder.is_dir());
        let reopened = local_host(root.path());
        assert_eq!(reopened.agents(), local.agents(), "the record survives a restart");
    }

    /// Everything the agent said in one turn.
    async fn said(p: &LinkedProvider, prompt: &str, model: &str) -> (String, Vec<StreamEvent>) {
        let events = collect(p.stream(&request(prompt, "chat-1", model)).await.unwrap()).await;
        let text = events.iter().filter(|e| e.event_type == StreamEventType::Text).map(|e| e.text.as_str()).collect();
        (text, events)
    }

    /// A "New Claude Code" hired on this computer starts in a folder of its
    /// own; told "work in <folder>", it moves there through the host's
    /// tool, the chat records the folder, and the next message runs there.
    #[tokio::test]
    async fn a_new_coding_agent_moves_to_the_folder_the_owner_names() {
        let root = tempfile::tempdir().unwrap();
        let local = local_host(root.path());
        let told_path = root.path().join("told.jsonl");
        let hosted = local.host(nebo_runtimes::acp::Agent::ClaudeCode, fake_acp(&told_path, "folders")).await.unwrap();
        let folder = root.path().join("home").join("NeboAI").join("claude-code").canonicalize().unwrap();
        assert_eq!(hosted.acp.workdir, folder, "a folder of its own, asked of nobody");
        let (store, _) = store_in(root.path());
        let relay = Relay::Hub { api_url: "http://127.0.0.1:9".into(), token: Arc::new(|| None) };
        let p = LinkedProvider::new(relay, store.clone(), Some(local.clone()), "Nebo on test");
        let model = format!("{SELF}/{}", hosted.id);

        let (text, _) = said(&p, "where", &model).await;
        assert_eq!(text, format!("Working in {}.", folder.display()));
        let chat = store.get_chat("chat-1").unwrap().unwrap();
        assert_eq!(chat.linked_folder.as_deref(), folder.to_str(), "the chat says where it works");
        let session = chat.linked_chat_id.clone().unwrap();

        let proj = root.path().join("home").join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let proj = proj.canonicalize().unwrap();
        let (text, events) = said(&p, &format!("work in {}", proj.display()), &model).await;
        assert!(text.contains("Now working in ~/proj."), "{text}");
        assert!(text.ends_with("Moved."), "{text}");
        assert!(events.iter().any(|e| e.tool_call.as_ref().is_some_and(|c| c.name == "move_to_folder")));
        let chat = store.get_chat("chat-1").unwrap().unwrap();
        assert_eq!(chat.linked_folder.as_deref(), proj.to_str(), "the chat records the move");
        assert_eq!(chat.linked_chat_id.as_deref(), Some(session.as_str()), "the same conversation");

        let (text, _) = said(&p, "where", &model).await;
        assert!(text.starts_with(&format!("Working in {}.", proj.display())), "the next message runs there: {text}");
        assert!(text.contains(&format!("Was working in {}.", folder.display())), "starting with the handoff: {text}");
    }

    /// One host per computer per OS user: while a nebo-link daemon is linked
    /// for this user, Nebo hosts nothing, a local employee reads the plain
    /// copy, and it never falls back to another brain.
    #[tokio::test]
    async fn nebo_hosts_nothing_while_nebo_link_hosts_this_computer() {
        let root = tempfile::tempdir().unwrap();
        let local = local_host(root.path());
        let daemon = root.path().join("nebo-link").join("b1");
        std::fs::create_dir_all(&daemon).unwrap();
        std::fs::write(daemon.join("link.json"), r#"{"botId":"b1","name":"Studio Mac"}"#).unwrap();

        assert_eq!(local.hosted_by_daemon().as_deref(), Some("Studio Mac"));
        assert!(local.oal().is_none());
        assert!(link_core::machine::hosting_app(&root.path().join("nebo-link")).is_none(), "Nebo records hosting nothing");
        assert!(local.hireable().is_empty());
        let refused = local
            .host(nebo_runtimes::acp::Agent::Codex, fake_acp(&root.path().join("told"), "git"))
            .await
            .unwrap_err();
        assert!(refused.contains("Studio Mac"), "{refused}");

        let (store, _) = store_in(root.path());
        let relay = Relay::Hub {
            api_url: "http://127.0.0.1:9".into(),
            token: Arc::new(|| None),
        };
        let p = LinkedProvider::new(relay, store, Some(local), "Nebo on test");
        let req = request("hello", "chat-1", &format!("{SELF}/claude-code"));
        let events = collect(p.stream(&req).await.unwrap()).await;
        assert_eq!(kinds(&events), vec![StreamEventType::Error, StreamEventType::Done]);
        assert_eq!(events[0].error.as_deref(), Some("Could not connect to Danny. Try again."));
    }
}
