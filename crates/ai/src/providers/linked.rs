//! The linked provider: the brain of an employee hired from a linked bot.
//!
//! A linked bot (a computer's agents joined to the owner's account by
//! `nebo-link`: OpenClaw, Hermes, Claude Code, Codex, ...) serves Nebo's chat
//! contract over its tunnel: the roster, chats and the streamed chat socket
//! the phone speaks. This provider drives one of its agents through that
//! contract, the way [`super::cli`] drives a CLI through a process: it
//! reports `handles_tools`, sends the turn, and maps the contract's events
//! into [`StreamEvent`]s.
//!
//! The target rides in the model id, `linked/<linkedBotId>/<agentId>`, on the
//! employee's `model_preference`; one provider serves every linked employee.
//! It reaches another computer's linked bot at
//! `{NEBOAI_API_URL}/t/{linkedBotId}/…` with the Nebo bot's own token, which
//! the hub admits for a bot of the same owner. An agent on this computer is
//! hosted by Nebo itself ([`super::local_host::LocalHost`]), under this bot's
//! own id: the same contract, carried in memory, with no hub between.
//!
//! The runtime keeps the transcript. One Nebo chat is one runtime session: the
//! first turn on a thread creates the runtime's chat and records its id on the
//! Nebo chat row (`chats.linked_chat_id`); every turn sends only the newest
//! user message, never the flattened history the CLI provider builds. Nebo's
//! system prompt and steering are not sent — the runtime owns its persona.
//!
//! A question the runtime stops for (`ask_request`, e.g. its permission to
//! run a command) becomes a [`StreamEvent::ask_request`] registered on the
//! run's ask channels, the ONE way a parked question is answered: the app's
//! ask card, the phone (live and on reload), and a loop reply all show it
//! with the runtime's own options, and the option chosen goes back as
//! `ask_response`.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use link_core::phone::{Contract, Outbound};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tracing::{info, warn};

use super::local_host::LocalHost;
use crate::types::*;

/// The provider id, and the prefix of every linked model id.
pub const ID: &str = "linked";

/// How long the hub and the link may take to answer a connect.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a cancel waits for the runtime's `chat_cancelled`.
const CANCEL_TIMEOUT: Duration = Duration::from_secs(10);

/// The Nebo bot's current NeboAI token, resolved per call (the hub rotates
/// it on every comms connect). `None` = not signed in.
pub type TokenSource = Arc<dyn Fn() -> Option<String> + Send + Sync>;

#[derive(Clone)]
pub struct LinkedProvider {
    /// `NEBOAI_API_URL`, without a trailing slash.
    api_url: String,
    store: Arc<db::Store>,
    token: TokenSource,
    client: reqwest::Client,
    /// This computer's host, when Nebo has a bot to host as.
    local: Option<Arc<LocalHost>>,
}

impl LinkedProvider {
    pub fn new(api_url: &str, store: Arc<db::Store>, token: TokenSource, local: Option<Arc<LocalHost>>) -> Self {
        Self {
            api_url: api_url.trim_end_matches('/').to_owned(),
            store,
            token,
            client: crate::http::request_client(),
            local,
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

    /// One turn, end to end. `Err` is the message the owner reads.
    async fn turn(
        &self,
        req: &ChatRequest,
        bot_id: &str,
        agent_id: &str,
        tx: &mpsc::Sender<StreamEvent>,
    ) -> Result<(), String> {
        let name = self.employee_name(req, agent_id);
        let offline = || format!("Could not connect to {name}. Try again.");
        let route = self.route(bot_id, &name)?;
        let prompt = owners_message(&req.messages).ok_or_else(|| "Nothing to send.".to_owned())?;

        let linked_chat_id = self
            .linked_chat(req, bot_id, agent_id, &route)
            .await
            .map_err(|e| match e {
                Reach::Offline => offline(),
                Reach::Refused(why) => why,
            })?;
        let session_id = format!("agent:{agent_id}:thread:{linked_chat_id}");

        let mut ws = self.socket(&route, bot_id).await.ok_or_else(offline)?;

        // The phone's handshake: `auth` is answered with `auth_ok` before
        // anything else.
        let token = match &route {
            Route::Hub { token } => token.as_str(),
            Route::Local(_) => "",
        };
        send(
            &mut ws,
            json!({ "type": "auth", "data": { "token": token } }),
            &offline,
        )
        .await?;
        loop {
            match tokio::time::timeout(CONNECT_TIMEOUT, ws.next()).await {
                Ok(Some(frame)) if frame["type"] == "auth_ok" => break,
                Ok(Some(_)) => continue,
                _ => return Err(offline()),
            }
        }

        // The employee's permission mode rides with the turn: a linked coding
        // agent runs it in its own matching mode (nebo-link maps it).
        let mut data = json!({
            "prompt": prompt,
            "agent_id": agent_id,
            "session_id": session_id,
        });
        if let Some(mode) = req.permission_mode {
            data["permission_mode"] = json!(mode.as_str());
        }
        send(
            &mut ws,
            json!({
                "type": "chat",
                "message_id": uuid::Uuid::new_v4().to_string(),
                "data": data,
            }),
            &offline,
        )
        .await?;
        info!(bot_id, agent_id, %session_id, prompt_len = prompt.len(), "linked: turn sent");

        // Answers to the runtime's questions arrive here from the ask door.
        let (answers_tx, mut answers_rx) = mpsc::channel::<(String, String)>(8);
        let cancel = req.cancel_token.clone().unwrap_or_default();
        let mut cancel_deadline: Option<tokio::time::Instant> = None;
        // A message sent into a turn another door started is queued behind
        // it; the running turn's events are not this one's.
        let mut queued = false;
        loop {
            tokio::select! {
                _ = cancel.cancelled(), if cancel_deadline.is_none() => {
                    send(&mut ws, json!({ "type": "cancel", "data": { "session_id": session_id } }), &offline).await?;
                    cancel_deadline = Some(tokio::time::Instant::now() + CANCEL_TIMEOUT);
                }
                _ = tokio::time::sleep_until(cancel_deadline.unwrap_or_else(tokio::time::Instant::now)), if cancel_deadline.is_some() => {
                    return Err("Cancelled".to_owned());
                }
                Some((request_id, value)) = answers_rx.recv() => {
                    send(&mut ws, json!({ "type": "ask_response", "data": { "request_id": request_id, "value": value } }), &offline).await?;
                }
                frame = ws.next() => {
                    let Some(frame) = frame else {
                        return Err(offline());
                    };
                    let data = &frame["data"];
                    if data["session_id"] != session_id {
                        continue;
                    }
                    let kind = frame["type"].as_str().unwrap_or("");
                    let terminal = matches!(kind, "chat_complete" | "chat_error" | "chat_cancelled");
                    if queued {
                        if terminal {
                            queued = false;
                        }
                        continue;
                    }
                    match kind {
                        "chat_stream" => {
                            if let Some(content) = data["content"].as_str().filter(|c| !c.is_empty()) {
                                let _ = tx.send(StreamEvent::text(content)).await;
                            }
                        }
                        "thinking" => {
                            if let Some(text) = data["text"].as_str().filter(|t| !t.is_empty()) {
                                let _ = tx.send(StreamEvent::thinking(text)).await;
                            }
                        }
                        "tool_start" => {
                            let _ = tx
                                .send(StreamEvent::tool_call(ToolCall {
                                    id: data["tool_id"].as_str().unwrap_or("").to_owned(),
                                    name: data["tool"].as_str().unwrap_or("").to_owned(),
                                    input: data["input"].clone(),
                                }))
                                .await;
                        }
                        "tool_result" => {
                            let is_error = data["is_error"].as_bool().unwrap_or(false);
                            let _ = tx
                                .send(StreamEvent {
                                    payload: None,
                                    provenance: None,
                                    event_type: StreamEventType::ToolResult,
                                    text: data["result"].as_str().unwrap_or("").to_owned(),
                                    tool_call: Some(ToolCall {
                                        id: data["tool_id"].as_str().unwrap_or("").to_owned(),
                                        name: data["tool_name"].as_str().unwrap_or("").to_owned(),
                                        input: Value::Null,
                                    }),
                                    error: is_error.then(|| "tool error".to_owned()),
                                    usage: None,
                                    rate_limit: None,
                                    widgets: None,
                                    provider_metadata: None,
                                    stop_reason: None,
                                    image_url: None,
                                })
                                .await;
                        }
                        "usage" => {
                            let _ = tx
                                .send(StreamEvent::usage(UsageInfo {
                                    input_tokens: data["input_tokens"].as_i64().unwrap_or(0) as i32,
                                    output_tokens: data["output_tokens"].as_i64().unwrap_or(0) as i32,
                                    ..UsageInfo::default()
                                }))
                                .await;
                        }
                        "ask_request" => {
                            self.ask(data, req, tx, &answers_tx).await;
                        }
                        "chat_complete" => {
                            let _ = tx.send(StreamEvent::done()).await;
                            return Ok(());
                        }
                        "chat_error" => {
                            if data["stop_reason"] == "queued_into_running_turn" {
                                queued = true;
                                continue;
                            }
                            let error = data["error"].as_str().filter(|e| !e.is_empty());
                            return Err(error.map(str::to_owned).unwrap_or_else(offline));
                        }
                        "chat_cancelled" => return Err("Cancelled".to_owned()),
                        _ => {}
                    }
                }
            }
        }
    }

    /// Where `bot_id` is reached: this computer's own host when it is this
    /// bot, else through the hub. `Err` is the message the owner reads.
    fn route(&self, bot_id: &str, name: &str) -> Result<Route, String> {
        if let Some(local) = self.local.as_ref().filter(|l| l.bot_id().as_deref() == Some(bot_id)) {
            return match local.contract() {
                Some(contract) => Ok(Route::Local(contract)),
                None => {
                    info!(bot_id, "linked: nebo-link hosts this computer's agents, so Nebo does not");
                    Err(format!("Could not connect to {name}. Try again."))
                }
            };
        }
        match (self.token)() {
            Some(token) => Ok(Route::Hub { token }),
            None => Err(format!("Sign in to NeboAI to reach {name}.")),
        }
    }

    /// The chat socket, through the hub or on this computer.
    async fn socket(&self, route: &Route, bot_id: &str) -> Option<Socket> {
        let token = match route {
            Route::Local(contract) => {
                return Some(Socket::Local {
                    frames: contract.subscribe(),
                    contract: contract.clone(),
                    replies: VecDeque::new(),
                });
            }
            Route::Hub { token } => token,
        };
        let url = format!("{}/t/{bot_id}/ws", ws_base(&self.api_url));
        let mut request = match url.as_str().into_client_request() {
            Ok(request) => request,
            Err(e) => {
                info!(bot_id, error = %e, "linked: the chat socket's address is not valid");
                return None;
            }
        };
        let Ok(bearer) = format!("Bearer {token}").parse() else {
            info!(bot_id, "linked: the NeboAI token is not a valid header value");
            return None;
        };
        request.headers_mut().insert(AUTHORIZATION, bearer);
        match tokio::time::timeout(CONNECT_TIMEOUT, tls::connect_ws(request)).await {
            Ok(Ok((ws, _))) => Some(Socket::Hub(Box::new(ws))),
            Ok(Err(e)) => {
                info!(bot_id, error = %e, "linked: the chat socket did not connect");
                None
            }
            Err(_) => None,
        }
    }

    /// The runtime's chat behind this Nebo chat: recorded on the chat row, or
    /// created on the thread's first turn and recorded then.
    async fn linked_chat(
        &self,
        req: &ChatRequest,
        bot_id: &str,
        agent_id: &str,
        route: &Route,
    ) -> Result<String, Reach> {
        if req.chat_id.is_empty() {
            return Err(Reach::Refused(
                "This turn belongs to no conversation, so it has no linked chat.".to_owned(),
            ));
        }
        let chat = self
            .store
            .get_chat(&req.chat_id)
            .map_err(|e| Reach::Refused(format!("Could not read the conversation: {e}")))?;
        if let Some(id) = chat
            .and_then(|c| c.linked_chat_id)
            .filter(|id| !id.is_empty())
        {
            return Ok(id);
        }
        let path = format!("/api/v1/agents/{agent_id}/chats");
        let (status, body) = match route {
            Route::Hub { token } => {
                let response = self
                    .client
                    .post(format!("{}/t/{bot_id}{path}", self.api_url))
                    .bearer_auth(token)
                    .json(&json!({}))
                    .send()
                    .await
                    .map_err(|e| {
                        info!(bot_id, error = %e, "linked: creating the runtime's chat did not connect");
                        Reach::Offline
                    })?;
                let status = response.status().as_u16();
                (status, response.json().await.unwrap_or(Value::Null))
            }
            Route::Local(contract) => match contract.rest("POST", &path).await {
                Ok(body) => (200, body),
                Err(refused) => (refused.status, json!({ "error": refused.message })),
            },
        };
        if !(200..300).contains(&status) {
            // 502 is the link saying the runtime is not answering; anything
            // else is a refusal with its own words.
            return Err(match body["error"].as_str().filter(|e| !e.is_empty()) {
                Some(error) if status != 502 => Reach::Refused(error.to_owned()),
                _ => Reach::Offline,
            });
        }
        let Some(id) = body["chat"]["id"].as_str().filter(|id| !id.is_empty()) else {
            return Err(Reach::Refused(
                "The linked bot created a chat without an id.".to_owned(),
            ));
        };
        self.store
            .set_chat_linked_chat_id(&req.chat_id, id)
            .map_err(|e| Reach::Refused(format!("Could not record the linked chat: {e}")))?;
        info!(bot_id, agent_id, chat_id = %req.chat_id, linked_chat_id = id, "linked: chat created");
        Ok(id.to_owned())
    }

    /// The runtime stopped to ask the owner: register the question on the
    /// run's ask channels and raise `ask_request` with the runtime's own
    /// options; the option chosen comes back through `answers`.
    async fn ask(
        &self,
        data: &Value,
        req: &ChatRequest,
        tx: &mpsc::Sender<StreamEvent>,
        answers: &mpsc::Sender<(String, String)>,
    ) {
        let request_id = data["request_id"].as_str().unwrap_or("").to_owned();
        let Some(channels) = req.ask_channels.as_ref() else {
            // No ask door on this run (nothing could answer): the question
            // stays open for the linked bot's own chat and the phone.
            warn!(request_id, "linked: an ask with no ask door on the run");
            return;
        };
        if request_id.is_empty() {
            return;
        }
        info!(request_id, "linked: the runtime asks the owner");
        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        channels.lock().await.insert(request_id.clone(), resp_tx);
        let _ = tx
            .send(StreamEvent::ask_request(
                request_id.clone(),
                data["prompt"].as_str().unwrap_or(""),
                Some(data["widgets"].clone()),
            ))
            .await;
        let answers = answers.clone();
        tokio::spawn(async move {
            // A question nobody answers (the run ended) is not answered: the
            // runtime cancels it with its turn.
            if let Ok(value) = resp_rx.await {
                let _ = answers.send((request_id, value)).await;
            }
        });
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

    /// The runtime keeps the transcript: a message is delivered once, and the
    /// provider answers only for the agent it is addressed to.
    fn retryable(&self) -> bool {
        false
    }

    async fn stream(&self, req: &ChatRequest) -> Result<EventReceiver, ProviderError> {
        // The runner hands the provider its model without the `linked/`
        // prefix, as it does every provider.
        let Some((bot_id, agent_id)) = split(&req.model) else {
            return Err(ProviderError::Request(format!(
                "not a linked agent: {:?}",
                req.model
            )));
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
    /// Another computer's linked bot, through the hub with the Nebo bot's
    /// token.
    Hub { token: String },
    /// This computer's own host.
    Local(Arc<Contract>),
}

/// The chat contract's socket: the phone's `{type, data}` frames, over the
/// hub's tunnel or in memory on this computer.
enum Socket {
    Hub(Box<WebSocketStream<MaybeTlsStream<TcpStream>>>),
    Local {
        contract: Arc<Contract>,
        frames: broadcast::Receiver<Outbound>,
        /// Direct answers to frames sent (`auth_ok`), read before the rest.
        replies: VecDeque<Value>,
    },
}

impl Socket {
    /// Sends one frame; `false` when the socket is gone.
    async fn send(&mut self, frame: Value) -> bool {
        match self {
            Socket::Hub(ws) => ws.send(WsMessage::text(frame.to_string())).await.is_ok(),
            Socket::Local { contract, replies, .. } => {
                replies.extend(contract.inbound(&frame));
                true
            }
        }
    }

    /// The next frame; `None` when the socket is gone.
    async fn next(&mut self) -> Option<Value> {
        match self {
            Socket::Hub(ws) => loop {
                match ws.next().await {
                    Some(Ok(WsMessage::Text(text))) => {
                        if let Ok(frame) = serde_json::from_str(text.as_str()) {
                            return Some(frame);
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) | None => return None,
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        info!(error = %e, "linked: the chat socket failed");
                        return None;
                    }
                }
            },
            Socket::Local { frames, replies, .. } => {
                if let Some(reply) = replies.pop_front() {
                    return Some(reply);
                }
                loop {
                    match frames.recv().await {
                        Ok(Outbound { kind, data }) => return Some(json!({ "type": kind, "data": data })),
                        // Fell behind: what comes next still arrives, as it
                        // does to a phone over the tunnel.
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => return None,
                    }
                }
            }
        }
    }
}

/// Why the linked bot could not be reached for a chat.
enum Reach {
    /// The link or the runtime is not answering; the owner reads
    /// "Could not connect to <name>. Try again."
    Offline,
    /// A refusal with its own words.
    Refused(String),
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

/// The socket base for an API base: `https://…` → `wss://…`.
fn ws_base(api_url: &str) -> String {
    if let Some(rest) = api_url.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = api_url.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        api_url.to_owned()
    }
}

async fn send(ws: &mut Socket, frame: Value, offline: &(dyn Fn() -> String + Sync)) -> Result<(), String> {
    if ws.send(frame).await { Ok(()) } else { Err(offline()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::sync::Mutex;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_util::sync::CancellationToken;

    const BOT: &str = "5a137883-0000-4000-8000-000000000001";

    /// What the fake linked bot does with a `chat` frame.
    #[derive(Clone, Copy)]
    enum Script {
        /// Text, a tool, usage, then completion.
        Turn,
        /// An ask; the answer's label is echoed in the reply.
        Ask,
        /// Streams one word, then waits for `cancel`.
        Hang,
    }

    #[derive(Default)]
    struct Recorded {
        chats_created: usize,
        bearers: Vec<String>,
        chat_frames: Vec<Value>,
        ask_responses: Vec<Value>,
        cancels: usize,
    }

    /// A fake contract behind `/t/<bot>/…`: `POST …/agents/{id}/chats` and
    /// the `/ws` socket, on one loopback port.
    async fn fake_link(script: Script) -> (String, Arc<Mutex<Recorded>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let recorded = Arc::new(Mutex::new(Recorded::default()));
        let rec = recorded.clone();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let rec = rec.clone();
                tokio::spawn(async move { serve(stream, script, rec).await });
            }
        });
        (url, recorded)
    }

    async fn serve(mut stream: TcpStream, script: Script, rec: Arc<Mutex<Recorded>>) {
        let mut head = [0u8; 2048];
        let n = stream.peek(&mut head).await.unwrap();
        let head = String::from_utf8_lossy(&head[..n]).into_owned();
        let target = head.split_whitespace().nth(1).unwrap_or("").to_owned();
        if target == format!("/t/{BOT}/ws") {
            let rec2 = rec.clone();
            let ws = tokio_tungstenite::accept_hdr_async(
                stream,
                move |req: &tokio_tungstenite::tungstenite::handshake::server::Request,
                      resp: tokio_tungstenite::tungstenite::handshake::server::Response| {
                    let bearer = req
                        .headers()
                        .get(AUTHORIZATION)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_owned();
                    rec2.lock().unwrap().bearers.push(bearer);
                    Ok(resp)
                },
            )
            .await
            .unwrap();
            serve_socket(ws, script, rec).await;
            return;
        }
        // REST: read the whole request, answer by path.
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let n = stream.read(&mut chunk).await.unwrap();
            buf.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&buf);
            if let Some((head, body)) = text.split_once("\r\n\r\n") {
                let len = head
                    .lines()
                    .find_map(|l| {
                        l.to_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0);
                if body.len() >= len || n == 0 {
                    break;
                }
            }
            if n == 0 {
                break;
            }
        }
        let bearer = head
            .lines()
            .find_map(|l| {
                l.to_lowercase()
                    .strip_prefix("authorization:")
                    .map(|v| v.trim().to_owned())
            })
            .unwrap_or_default();
        let (status, body) = if target == format!("/t/{BOT}/api/v1/agents/coder/chats")
            && head.starts_with("POST")
        {
            let mut r = rec.lock().unwrap();
            r.chats_created += 1;
            r.bearers.push(bearer);
            let id = format!("api_{}", r.chats_created);
            (
                "200 OK",
                json!({ "chat": { "id": id, "title": "New chat" } }),
            )
        } else {
            ("404 Not Found", json!({ "error": "not on a linked bot" }))
        };
        let body = body.to_string();
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        stream.shutdown().await.ok();
    }

    async fn serve_socket(
        mut ws: tokio_tungstenite::WebSocketStream<TcpStream>,
        script: Script,
        rec: Arc<Mutex<Recorded>>,
    ) {
        let mut session = String::new();
        let payload = |session: &str, fields: Value| {
            let mut data = json!({ "agent_id": "coder", "session_id": session, "turn_id": "t1" });
            if let (Value::Object(data), Value::Object(fields)) = (&mut data, fields) {
                data.extend(fields);
            }
            data
        };
        while let Some(Ok(WsMessage::Text(text))) = ws.next().await {
            let frame: Value = serde_json::from_str(text.as_str()).unwrap();
            match frame["type"].as_str().unwrap_or("") {
                "auth" => {
                    ws.send(WsMessage::text(json!({ "type": "auth_ok" }).to_string()))
                        .await
                        .unwrap();
                }
                "chat" => {
                    rec.lock().unwrap().chat_frames.push(frame.clone());
                    session = frame["data"]["session_id"]
                        .as_str()
                        .unwrap_or("")
                        .to_owned();
                    let out = |kind: &str, fields: Value| {
                        let f = json!({ "type": kind, "data": payload(&session, fields) });
                        f.to_string()
                    };
                    let frames: Vec<String> = match script {
                        Script::Turn => vec![
                            out("thinking", json!({ "text": "hmm" })),
                            out(
                                "chat_stream",
                                json!({ "content": "Listing ", "done": false }),
                            ),
                            out(
                                "tool_start",
                                json!({ "tool_id": "call_1", "tool": "terminal", "label": "terminal", "input": { "command": "ls" } }),
                            ),
                            out(
                                "tool_result",
                                json!({ "tool_id": "call_1", "tool_name": "terminal", "result": "a b", "is_error": false, "outcome": "terminal" }),
                            ),
                            out("chat_stream", json!({ "content": "done.", "done": false })),
                            out("usage", json!({ "input_tokens": 12, "output_tokens": 5 })),
                            out(
                                "chat_complete",
                                json!({ "stop_reason": "end_turn", "message_id": "t1" }),
                            ),
                        ],
                        Script::Ask => vec![out(
                            "ask_request",
                            json!({
                                "request_id": "req-9",
                                "prompt": "Run `rm -rf build`?",
                                "widgets": [{ "type": "options", "multiSelect": false, "options": ["Allow once", "Always allow", "Deny"] }],
                            }),
                        )],
                        Script::Hang => vec![out(
                            "chat_stream",
                            json!({ "content": "Working", "done": false }),
                        )],
                    };
                    for f in frames {
                        ws.send(WsMessage::text(f)).await.unwrap();
                    }
                }
                "ask_response" => {
                    rec.lock()
                        .unwrap()
                        .ask_responses
                        .push(frame["data"].clone());
                    let value = frame["data"]["value"].as_str().unwrap_or("").to_owned();
                    let f = json!({ "type": "chat_stream", "data": payload(&session, json!({ "content": format!("answered: {value}") })) });
                    ws.send(WsMessage::text(f.to_string())).await.unwrap();
                    let f = json!({ "type": "chat_complete", "data": payload(&session, json!({ "stop_reason": "end_turn" })) });
                    ws.send(WsMessage::text(f.to_string())).await.unwrap();
                }
                "cancel" => {
                    rec.lock().unwrap().cancels += 1;
                    let f =
                        json!({ "type": "chat_cancelled", "data": payload(&session, Value::Null) });
                    ws.send(WsMessage::text(f.to_string())).await.unwrap();
                }
                _ => {}
            }
        }
    }

    fn store() -> (tempfile::TempDir, Arc<db::Store>) {
        let dir = tempfile::tempdir().unwrap();
        let store = db::Store::new(&dir.path().join("linked.db").to_string_lossy()).unwrap();
        store
            .create_agent(
                "emp-1",
                Some("linked"),
                "Danny",
                "",
                "---\nname: Danny\n---\n",
                "{}",
                None,
                None,
            )
            .unwrap();
        store.create_chat("chat-1", "First").unwrap();
        (dir, Arc::new(store))
    }

    fn provider(url: &str, store: Arc<db::Store>) -> LinkedProvider {
        LinkedProvider::new(url, store, Arc::new(|| Some("bot-jwt".to_owned())), None)
    }

    fn request(prompt: &str, chat_id: &str) -> ChatRequest {
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
            model: format!("{BOT}/coder"),
            chat_id: chat_id.into(),
            system: "NEVER SENT".into(),
            ..ChatRequest::new(RequestTrace {
                agent_id: "emp-1".into(),
                ..RequestTrace::new("agent_turn")
            })
        }
    }

    async fn collect(mut rx: EventReceiver) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        while let Some(e) = rx.recv().await {
            events.push(e);
        }
        events
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
        assert_eq!(ws_base("https://api.neboai.com"), "wss://api.neboai.com");
        assert_eq!(ws_base("http://127.0.0.1:1"), "ws://127.0.0.1:1");
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

    /// The first turn creates the runtime's chat with the bot token and
    /// records it; the second reuses it, and each turn sends exactly the
    /// newest user message — never the history, never the system prompt.
    #[tokio::test]
    async fn one_nebo_chat_is_one_linked_chat_and_only_the_newest_message_travels() {
        let (url, rec) = fake_link(Script::Turn).await;
        let (_dir, store) = store();
        let p = provider(&url, store.clone());

        let first = collect(
            p.stream(&request("list the files", "chat-1"))
                .await
                .unwrap(),
        )
        .await;
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
            ]
        );
        assert_eq!(first[1].text, "Listing ");
        let call = first[2].tool_call.as_ref().unwrap();
        assert_eq!(
            (call.id.as_str(), call.name.as_str()),
            ("call_1", "terminal")
        );
        assert_eq!(call.input["command"], "ls");
        assert_eq!(first[3].text, "a b");
        assert_eq!(first[3].tool_call.as_ref().unwrap().id, "call_1");
        let usage = first[5].usage.as_ref().unwrap();
        assert_eq!((usage.input_tokens, usage.output_tokens), (12, 5));
        assert_eq!(
            store
                .get_chat("chat-1")
                .unwrap()
                .unwrap()
                .linked_chat_id
                .as_deref(),
            Some("api_1")
        );

        let mut in_a_run = request("and now?", "chat-1");
        in_a_run.permission_mode = Some(types::permissions::Mode::FullAccess);
        let second = collect(p.stream(&in_a_run).await.unwrap()).await;
        assert_eq!(second.last().unwrap().event_type, StreamEventType::Done);

        let r = rec.lock().unwrap();
        assert_eq!(r.chats_created, 1, "the second turn reuses the linked chat");
        assert_eq!(
            r.bearers.len(),
            3,
            "one chat creation and two sockets: {:?}",
            r.bearers
        );
        assert!(
            r.bearers
                .iter()
                .all(|b| b.eq_ignore_ascii_case("Bearer bot-jwt")),
            "{:?}",
            r.bearers
        );
        assert_eq!(r.chat_frames.len(), 2);
        assert_eq!(r.chat_frames[0]["data"].get("permission_mode"), None, "a call outside a run names none");
        assert_eq!(r.chat_frames[1]["data"]["permission_mode"], "full_access", "the run's mode rides with the turn");
        for (frame, prompt) in r.chat_frames.iter().zip(["list the files", "and now?"]) {
            assert_eq!(frame["data"]["prompt"], prompt);
            assert_eq!(frame["data"]["agent_id"], "coder");
            assert_eq!(frame["data"]["session_id"], "agent:coder:thread:api_1");
            assert!(frame["message_id"].as_str().is_some_and(|m| !m.is_empty()));
            let wire = frame.to_string();
            assert!(
                !wire.contains("earlier") && !wire.contains("NEVER SENT"),
                "{wire}"
            );
        }
    }

    /// An ask becomes an `ask_request` on the run's ask channels with the
    /// runtime's own options; the option answered there goes back as it is.
    #[tokio::test]
    async fn an_ask_round_trips_through_the_ask_channels() {
        let (url, rec) = fake_link(Script::Ask).await;
        let (_dir, store) = store();
        let p = provider(&url, store);
        let channels: AskChannels = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let mut req = request("clean the build", "chat-1");
        req.ask_channels = Some(channels.clone());

        let mut rx = p.stream(&req).await.unwrap();
        let ask = rx.recv().await.unwrap();
        assert_eq!(ask.event_type, StreamEventType::AskRequest);
        assert_eq!(ask.error.as_deref(), Some("req-9"), "the question's id");
        assert_eq!(ask.text, "Run `rm -rf build`?");
        let widgets = ask.widgets.as_ref().unwrap();
        assert_eq!(widgets[0]["options"], json!(["Allow once", "Always allow", "Deny"]));

        // The ask card's answer, through the ONE pathway.
        let sender = channels
            .lock()
            .await
            .remove("req-9")
            .expect("registered on the run");
        sender.send("Always allow".to_owned()).unwrap();

        let rest = collect(rx).await;
        assert_eq!(
            kinds(&rest),
            vec![StreamEventType::Text, StreamEventType::Done]
        );
        assert_eq!(rest[0].text, "answered: Always allow");
        let r = rec.lock().unwrap();
        assert_eq!(r.ask_responses.len(), 1);
        assert_eq!(r.ask_responses[0]["request_id"], "req-9");
        assert_eq!(r.ask_responses[0]["value"], "Always allow");
    }

    /// A stop sends the contract's `cancel`, waits for `chat_cancelled`, and
    /// ends the stream the way the CLI provider does.
    #[tokio::test]
    async fn cancel_reaches_the_runtime_and_ends_the_turn() {
        let (url, rec) = fake_link(Script::Hang).await;
        let (_dir, store) = store();
        let p = provider(&url, store);
        let token = CancellationToken::new();
        let mut req = request("do something long", "chat-1");
        req.cancel_token = Some(token.clone());

        let mut rx = p.stream(&req).await.unwrap();
        let first = rx.recv().await.unwrap();
        assert_eq!(first.text, "Working");
        token.cancel();
        let rest = collect(rx).await;
        assert_eq!(
            kinds(&rest),
            vec![StreamEventType::Error, StreamEventType::Done]
        );
        assert_eq!(rest[0].error.as_deref(), Some("Cancelled"));
        assert_eq!(rec.lock().unwrap().cancels, 1);
    }

    /// Nothing answers at the linked bot: the plain copy, no retry.
    #[tokio::test]
    async fn an_unreachable_link_answers_with_the_plain_copy() {
        let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", closed.local_addr().unwrap());
        drop(closed);
        let (_dir, store) = store();
        let p = provider(&url, store);
        assert!(!p.retryable());

        let events = collect(p.stream(&request("hello", "chat-1")).await.unwrap()).await;
        assert_eq!(
            kinds(&events),
            vec![StreamEventType::Error, StreamEventType::Done]
        );
        assert_eq!(
            events[0].error.as_deref(),
            Some("Could not connect to Danny. Try again.")
        );
    }

    // -- A coding agent on this computer ----------------------------------

    /// This bot's id, under which Nebo hosts this computer's agents.
    const SELF: &str = "5e1f0000-0000-4000-8000-000000000002";

    /// Not a test when the harness runs it: the ACP agent a local hire
    /// starts (this test binary again, with `NEBO_FAKE_ACP` naming the file
    /// it writes what it was told to). It asks before it runs `git status`.
    #[test]
    fn fake_acp_agent() {
        use std::io::{BufRead, Write};
        let Ok(told) = std::env::var("NEBO_FAKE_ACP") else {
            return;
        };
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
        // The harness printed "test ... " without a newline: end that line,
        // so every frame is a line of its own.
        send(Value::Null);
        let mut prompt_id = Value::Null;
        for line in std::io::stdin().lock().lines() {
            let Ok(message) = serde_json::from_str::<Value>(&line.unwrap()) else {
                continue;
            };
            let id = message["id"].clone();
            let reply = |result: Value| send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
            match message["method"].as_str() {
                Some("initialize") => reply(json!({
                    "protocolVersion": 1,
                    "agentCapabilities": { "loadSession": false },
                    "agentInfo": { "name": "fake-acp" },
                })),
                Some("session/new") => reply(json!({
                    "sessionId": "s-1",
                    "modes": { "currentModeId": "default", "availableModes": [
                        { "id": "default", "name": "Default", "_meta": { "kind": "standard" } },
                        { "id": "acceptEdits", "name": "Accept edits", "_meta": { "kind": "standard" } },
                    ] },
                })),
                Some("session/set_mode") => {
                    note(json!({ "mode": message["params"]["modeId"] }));
                    reply(json!({}));
                }
                Some("session/prompt") => {
                    note(json!({ "prompt": message["params"]["prompt"][0]["text"] }));
                    prompt_id = id.clone();
                    update(json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "Checking." } }));
                    send(json!({ "jsonrpc": "2.0", "id": 900, "method": "session/request_permission", "params": {
                        "sessionId": "s-1",
                        "toolCall": { "toolCallId": "call_1", "title": "git status", "kind": "execute", "status": "pending", "rawInput": { "command": "git status" } },
                        "options": [
                            { "optionId": "allow", "name": "Allow", "kind": "allow_once" },
                            { "optionId": "reject", "name": "Reject", "kind": "reject_once" },
                        ],
                    } }));
                }
                None if id == json!(900) => {
                    note(json!({ "answer": message["result"]["outcome"] }));
                    update(json!({ "sessionUpdate": "tool_call_update", "toolCallId": "call_1", "status": "completed",
                        "content": [{ "type": "content", "content": { "type": "text", "text": "nothing to commit" } }] }));
                    update(json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "Ran it." } }));
                    send(json!({ "jsonrpc": "2.0", "id": prompt_id, "result": { "stopReason": "end_turn", "usage": { "inputTokens": 7, "outputTokens": 3 } } }));
                }
                _ => {}
            }
        }
    }

    /// How a local hire starts the fake agent.
    fn fake_acp(told: &std::path::Path) -> nebo_runtimes::RuntimeCommand {
        nebo_runtimes::RuntimeCommand {
            program: std::env::current_exe().unwrap().to_string_lossy().into_owned(),
            args: ["providers::linked::tests::fake_acp_agent", "--exact", "--nocapture", "--test-threads=1"]
                .map(String::from)
                .to_vec(),
            env: vec![("NEBO_FAKE_ACP".into(), told.to_string_lossy().into_owned())],
        }
    }

    fn local_host(root: &std::path::Path) -> Arc<LocalHost> {
        LocalHost::open(
            Arc::new(|| Some(SELF.to_owned())),
            root.join("link"),
            root.join("home"),
            Some(root.join("nebo-link")),
        )
    }

    /// A coding agent hired on this computer runs in Nebo, in its own
    /// folder: a turn reaches it, its permission request is an ask on the
    /// run's ask channels with its own options in the owner's words, the
    /// answer goes back, and the turn completes in the employee's mode. No
    /// hub is reachable and no NeboAI token exists: none is needed.
    #[tokio::test]
    async fn a_coding_agent_on_this_computer_runs_in_nebo_with_no_hub() {
        let root = tempfile::tempdir().unwrap();
        let told = root.path().join("told.jsonl");
        let local = local_host(root.path());
        let hosted = local.host(nebo_runtimes::acp::Agent::ClaudeCode, fake_acp(&told)).await.unwrap();
        assert_eq!((hosted.id.as_str(), hosted.label.as_str()), ("claude-code", "Claude Code"));
        let folder = root.path().join("home").join("NeboAI").join("claude-code").canonicalize().unwrap();
        assert_eq!(hosted.acp.workdir, folder, "its own folder under ~/NeboAI");
        let second = local.host(nebo_runtimes::acp::Agent::ClaudeCode, fake_acp(&told)).await.unwrap();
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
        let (_dir, store) = store();
        let p = LinkedProvider::new(&hub, store.clone(), Arc::new(|| None), Some(local.clone()));
        let channels: AskChannels = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let mut req = request("check the repo", "chat-1");
        req.model = format!("{SELF}/{}", hosted.id);
        req.permission_mode = Some(types::permissions::Mode::Automatic);
        req.ask_channels = Some(channels.clone());

        let mut rx = p.stream(&req).await.unwrap();
        let mut before = Vec::new();
        let ask = loop {
            let event = tokio::time::timeout(Duration::from_secs(60), rx.recv()).await.unwrap().unwrap();
            if event.event_type == StreamEventType::AskRequest {
                break event;
            }
            assert_ne!(event.event_type, StreamEventType::Error, "{:?}", event.error);
            before.push(event);
        };
        assert_eq!(kinds(&before), vec![StreamEventType::Text, StreamEventType::ToolCall]);
        assert_eq!(before[0].text, "Checking.");
        assert_eq!(before[1].tool_call.as_ref().unwrap().name, "git status");
        assert_eq!(ask.error.as_deref(), Some("call_1"), "the question's id");
        assert_eq!(ask.text, "git status");
        assert_eq!(ask.widgets.as_ref().unwrap()[0]["options"], json!(["Allow once", "Deny"]));

        // The ask card's answer, through the ONE pathway.
        channels.lock().await.remove("call_1").expect("registered on the run").send("Allow once".to_owned()).unwrap();
        let rest = tokio::time::timeout(Duration::from_secs(60), collect(rx)).await.unwrap();
        assert_eq!(
            kinds(&rest),
            vec![StreamEventType::ToolResult, StreamEventType::Text, StreamEventType::Usage, StreamEventType::Done]
        );
        assert_eq!(rest[0].text, "nothing to commit");
        assert_eq!(rest[1].text, "Ran it.");
        let usage = rest[2].usage.as_ref().unwrap();
        assert_eq!((usage.input_tokens, usage.output_tokens), (7, 3));

        let told: Vec<Value> = std::fs::read_to_string(&told)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(
            told,
            vec![
                json!({ "mode": "acceptEdits" }),
                json!({ "prompt": "check the repo" }),
                json!({ "answer": { "outcome": "selected", "optionId": "allow" } }),
            ],
            "the employee's mode, then only the newest message, then the owner's answer"
        );
        assert_eq!(
            store.get_chat("chat-1").unwrap().unwrap().linked_chat_id.as_deref(),
            Some("claude-code~s-1"),
            "the Nebo chat records the agent's session"
        );

        // Fired: the agent is no longer hosted, and its folder stays.
        local.remove(&hosted.id).unwrap();
        assert_eq!(local.agents().iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["claude-code-2"]);
        assert_eq!(link_core::machine::hosting_app(&daemon_home).unwrap().agents, ["Claude Code 2"]);
        assert!(folder.is_dir());
        let reopened = local_host(root.path());
        assert_eq!(reopened.agents(), local.agents(), "the record survives a restart");
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
        assert!(local.contract().is_none());
        assert!(
            link_core::machine::hosting_app(&root.path().join("nebo-link")).is_none(),
            "Nebo records hosting nothing"
        );
        assert!(local.hireable().is_empty());
        let refused = local
            .host(nebo_runtimes::acp::Agent::Codex, fake_acp(&root.path().join("told")))
            .await
            .unwrap_err();
        assert!(refused.contains("Studio Mac"), "{refused}");

        let (_dir, store) = store();
        let p = LinkedProvider::new("http://127.0.0.1:9", store, Arc::new(|| None), Some(local));
        let mut req = request("hello", "chat-1");
        req.model = format!("{SELF}/claude-code");
        let events = collect(p.stream(&req).await.unwrap()).await;
        assert_eq!(kinds(&events), vec![StreamEventType::Error, StreamEventType::Done]);
        assert_eq!(events[0].error.as_deref(), Some("Could not connect to Danny. Try again."));
    }
}
