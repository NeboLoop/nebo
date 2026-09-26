//! The linked provider: the brain of an employee hired from a linked bot.
//!
//! A linked bot (an OpenClaw or Hermes install joined to the owner's account
//! by `nebo-link`) serves Nebo's chat contract over its tunnel: the roster,
//! chats and the streamed chat socket the phone speaks. This provider drives
//! one of its agents through that contract, the way [`super::cli`] drives a
//! CLI through a process: it reports `handles_tools`, sends the turn, and
//! maps the contract's events into [`StreamEvent`]s.
//!
//! The target rides in the model id, `linked/<linkedBotId>/<agentId>`, on the
//! employee's `model_preference`; one provider serves every linked employee.
//! It reaches the linked bot at `{NEBOAI_API_URL}/t/{linkedBotId}/…` with the
//! Nebo bot's own token, which the hub admits for a bot of the same owner.
//!
//! The runtime keeps the transcript. One Nebo chat is one runtime session: the
//! first turn on a thread creates the runtime's chat and records its id on the
//! Nebo chat row (`chats.linked_chat_id`); every turn sends only the newest
//! user message, never the flattened history the CLI provider builds. Nebo's
//! system prompt and steering are not sent — the runtime owns its persona.
//!
//! An approval the runtime stops for (`ask_request`) becomes a
//! [`StreamEvent::approval_request`] registered on the run's approval
//! channels, the ONE tool-approval pathway, so the ApprovalGate, the phone and
//! the comm relay answer it; the decision goes back as `ask_response`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tracing::{info, warn};

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

pub struct LinkedProvider {
    /// `NEBOAI_API_URL`, without a trailing slash.
    api_url: String,
    store: Arc<db::Store>,
    token: TokenSource,
    client: reqwest::Client,
}

impl LinkedProvider {
    pub fn new(api_url: &str, store: Arc<db::Store>, token: TokenSource) -> Self {
        Self {
            api_url: api_url.trim_end_matches('/').to_owned(),
            store,
            token,
            client: crate::http::request_client(),
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
        let Some(token) = (self.token)() else {
            return Err(format!("Sign in to NeboAI to reach {name}."));
        };
        let prompt = owners_message(&req.messages).ok_or_else(|| "Nothing to send.".to_owned())?;

        let linked_chat_id = self
            .linked_chat(req, bot_id, agent_id, &token)
            .await
            .map_err(|e| match e {
                Reach::Offline => offline(),
                Reach::Refused(why) => why,
            })?;
        let session_id = format!("agent:{agent_id}:thread:{linked_chat_id}");

        let url = format!("{}/t/{bot_id}/ws", ws_base(&self.api_url));
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|e| format!("{}: {e}", offline()))?;
        let bearer = format!("Bearer {token}")
            .parse()
            .map_err(|_| "The NeboAI token is not a valid header value.".to_owned())?;
        request.headers_mut().insert(AUTHORIZATION, bearer);
        let connect = tls::connect_ws(request);
        let (mut ws, _) = match tokio::time::timeout(CONNECT_TIMEOUT, connect).await {
            Ok(Ok(connected)) => connected,
            Ok(Err(e)) => {
                info!(bot_id, error = %e, "linked: the chat socket did not connect");
                return Err(offline());
            }
            Err(_) => return Err(offline()),
        };

        // The phone's handshake: `auth` is answered with `auth_ok` before
        // anything else.
        send(
            &mut ws,
            json!({ "type": "auth", "data": { "token": token } }),
            &offline,
        )
        .await?;
        loop {
            let frame = match tokio::time::timeout(CONNECT_TIMEOUT, ws.next()).await {
                Ok(Some(Ok(WsMessage::Text(text)))) => text,
                Ok(Some(Ok(_))) => continue,
                _ => return Err(offline()),
            };
            match serde_json::from_str::<Value>(frame.as_str()) {
                Ok(v) if v["type"] == "auth_ok" => break,
                _ => continue,
            }
        }

        send(
            &mut ws,
            json!({
                "type": "chat",
                "message_id": uuid::Uuid::new_v4().to_string(),
                "data": {
                    "prompt": prompt,
                    "agent_id": agent_id,
                    "session_id": session_id,
                },
            }),
            &offline,
        )
        .await?;
        info!(bot_id, agent_id, %session_id, prompt_len = prompt.len(), "linked: turn sent");

        // Decisions on the runtime's asks arrive here from the approval door.
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
                    let text = match frame {
                        Some(Ok(WsMessage::Text(text))) => text,
                        Some(Ok(WsMessage::Close(_))) | None => return Err(offline()),
                        Some(Ok(_)) => continue,
                        Some(Err(e)) => {
                            info!(bot_id, error = %e, "linked: the chat socket failed");
                            return Err(offline());
                        }
                    };
                    let Ok(frame) = serde_json::from_str::<Value>(text.as_str()) else {
                        continue;
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

    /// The runtime's chat behind this Nebo chat: recorded on the chat row, or
    /// created on the thread's first turn and recorded then.
    async fn linked_chat(
        &self,
        req: &ChatRequest,
        bot_id: &str,
        agent_id: &str,
        token: &str,
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
        let url = format!("{}/t/{bot_id}/api/v1/agents/{agent_id}/chats", self.api_url);
        let response = self
            .client
            .post(&url)
            .bearer_auth(token)
            .json(&json!({}))
            .send()
            .await
            .map_err(|e| {
                info!(bot_id, error = %e, "linked: creating the runtime's chat did not connect");
                Reach::Offline
            })?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            // 502 is the link saying the runtime is not answering; anything
            // else is a refusal with its own words.
            return Err(match body["error"].as_str().filter(|e| !e.is_empty()) {
                Some(error) if status != reqwest::StatusCode::BAD_GATEWAY => {
                    Reach::Refused(error.to_owned())
                }
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

    /// The runtime stopped for the owner: register the request on the run's
    /// approval channels and raise `approval_request`; the decision comes back
    /// through `answers` as the label of one of the ask's options.
    async fn ask(
        &self,
        data: &Value,
        req: &ChatRequest,
        tx: &mpsc::Sender<StreamEvent>,
        answers: &mpsc::Sender<(String, String)>,
    ) {
        let request_id = data["request_id"].as_str().unwrap_or("").to_owned();
        let prompt = data["prompt"].as_str().unwrap_or("").to_owned();
        let options: Vec<String> = data["widgets"][0]["options"]
            .as_array()
            .map(|opts| {
                opts.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let Some(channels) = req.approval_channels.as_ref() else {
            // No approval door on this run (nothing could answer): the ask
            // stays open for the linked bot's own chat and the phone.
            warn!(
                request_id,
                "linked: an ask with no approval door on the run"
            );
            return;
        };
        if request_id.is_empty() {
            return;
        }
        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        channels.lock().await.insert(request_id.clone(), resp_tx);
        let _ = tx
            .send(StreamEvent::approval_request(ToolCall {
                id: request_id.clone(),
                name: prompt,
                input: json!({ "options": options }),
            }))
            .await;
        let answers = answers.clone();
        tokio::spawn(async move {
            let decision = resp_rx.await.unwrap_or_else(|_| "deny".to_owned());
            let value = option_for(&decision, &options);
            let _ = answers.send((request_id, value)).await;
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
        let this = Self {
            api_url: self.api_url.clone(),
            store: self.store.clone(),
            token: self.token.clone(),
            client: self.client.clone(),
        };
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

/// Why the linked bot could not be reached for a chat.
enum Reach {
    /// The link or the runtime is not answering; the owner reads
    /// "Could not connect to <name>. Try again."
    Offline,
    /// A refusal with its own words.
    Refused(String),
}

/// The ask option a Nebo decision (`once` / `always` / `deny`) answers with.
/// The options are the contract's labels ("Allow once", "Always allow",
/// "Deny"); a runtime that offers no "always" gets "once".
fn option_for(decision: &str, options: &[String]) -> String {
    let find = |word: &str| {
        options
            .iter()
            .find(|o| o.to_lowercase().contains(word))
            .cloned()
    };
    match decision {
        "deny" => find("deny").or_else(|| options.last().cloned()),
        "always" => find("always")
            .or_else(|| find("once"))
            .or_else(|| options.first().cloned()),
        _ => find("once").or_else(|| options.first().cloned()),
    }
    .unwrap_or_else(|| decision.to_owned())
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

async fn send<S>(
    ws: &mut S,
    frame: Value,
    offline: &(dyn Fn() -> String + Sync),
) -> Result<(), String>
where
    S: futures::Sink<WsMessage> + Unpin,
{
    ws.send(WsMessage::text(frame.to_string()))
        .await
        .map_err(|_| offline())
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
        LinkedProvider::new(url, store, Arc::new(|| Some("bot-jwt".to_owned())))
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

    #[test]
    fn decisions_pick_the_contracts_labels() {
        let full: Vec<String> = ["Allow once", "Always allow", "Deny"]
            .map(String::from)
            .into();
        assert_eq!(option_for("once", &full), "Allow once");
        assert_eq!(option_for("always", &full), "Always allow");
        assert_eq!(option_for("deny", &full), "Deny");
        let smart: Vec<String> = ["Allow once", "Deny"].map(String::from).into();
        assert_eq!(option_for("always", &smart), "Allow once");
        assert_eq!(option_for("deny", &[]), "deny");
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

        let second = collect(p.stream(&request("and now?", "chat-1")).await.unwrap()).await;
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

    /// An ask becomes an `approval_request` on the run's approval channels;
    /// the decision answered there goes back as the option's label.
    #[tokio::test]
    async fn an_ask_round_trips_through_the_approval_channels() {
        let (url, rec) = fake_link(Script::Ask).await;
        let (_dir, store) = store();
        let p = provider(&url, store);
        let channels: ApprovalChannels = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let mut req = request("clean the build", "chat-1");
        req.approval_channels = Some(channels.clone());

        let mut rx = p.stream(&req).await.unwrap();
        let ask = rx.recv().await.unwrap();
        assert_eq!(ask.event_type, StreamEventType::ApprovalRequest);
        let call = ask.tool_call.as_ref().unwrap();
        assert_eq!(call.id, "req-9");
        assert_eq!(call.name, "Run `rm -rf build`?");
        assert_eq!(call.input["options"][1], "Always allow");

        // The ApprovalGate's answer, through the ONE pathway.
        let sender = channels
            .lock()
            .await
            .remove("req-9")
            .expect("registered on the run");
        sender.send("always".to_owned()).unwrap();

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
}
