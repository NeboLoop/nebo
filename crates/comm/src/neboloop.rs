//! NeboLoop WebSocket plugin — implements `CommPlugin` for the NeboLoop comms
//! gateway. Connects via tokio-tungstenite, authenticates with binary framing,
//! and dispatches typed messages (installs, chat, DMs, loop channels, voice).

use std::collections::HashMap;
use std::sync::Arc;

use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::MaybeTlsStream;
use tracing::{debug, info, warn};

use crate::api::NeboLoopApi;
use crate::compress;
use crate::dedup::DedupWindow;
use crate::frame::{self, Header};
use crate::ulid::UlidGen;
use crate::wire;
use crate::{
    AgentCard, ChannelMemberItem, ChannelMessageItem, CommError, CommMessage,
    CommMessageType, CommPlugin, LoopChannelInfo, LoopInfo, MessageHandler,
};

type WsStream = tokio_tungstenite::WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Channel metadata tracked after JOIN responses.
#[derive(Debug, Clone)]
pub struct ChannelMeta {
    pub channel_id: String,
    pub channel_name: String,
    pub loop_id: String,
}

/// DM peer tracked after JOIN responses.
#[derive(Debug, Clone)]
pub struct DmPeer {
    pub peer_id: String,
    pub peer_type: String, // "bot" or "person"
    pub loop_id: String,
}

struct Inner {
    connected: bool,
    handler: Option<MessageHandler>,
    send_tx: Option<mpsc::Sender<Vec<u8>>>,
    cancel: Option<tokio_util::sync::CancellationToken>,
    api: Option<Arc<NeboLoopApi>>,
}

/// NeboLoop WebSocket CommPlugin.
pub struct NeboLoopPlugin {
    inner: RwLock<Inner>,
    bot_id: RwLock<String>,
    /// Rotated bot JWT from the last AUTH_OK (token rotation).
    rotated_token: RwLock<Option<String>>,
    /// Conversation maps — updated by the join processor, queried by public methods.
    conv_maps: Arc<RwLock<ConvMaps>>,
    /// Monotonic ULID generator for outgoing message IDs.
    ulid_gen: UlidGen,
}

impl NeboLoopPlugin {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Inner {
                connected: false,
                handler: None,
                send_tx: None,
                cancel: None,
                api: None,
            }),
            bot_id: RwLock::new(String::new()),
            rotated_token: RwLock::new(None),
            conv_maps: Arc::new(RwLock::new(ConvMaps::default())),
            ulid_gen: UlidGen::new(),
        }
    }

    /// Returns the rotated bot JWT from the last AUTH_OK, if any.
    /// The caller should persist this token and use it for the next connect.
    pub async fn take_rotated_token(&self) -> Option<String> {
        self.rotated_token.write().await.take()
    }

    /// Get the API client (available after connect).
    pub async fn api(&self) -> Option<Arc<NeboLoopApi>> {
        self.inner.read().await.api.clone()
    }

    /// Get the conversation ID for a given key (e.g. "botId:chat").
    pub async fn conversation_for_key(&self, key: &str) -> Option<String> {
        self.conv_maps.read().await.conv_by_key.get(key).cloned()
    }

    /// Get the conversation ID for a channel.
    pub async fn conversation_for_channel(&self, channel_id: &str) -> Option<String> {
        self.conv_maps.read().await.channel_convs.get(channel_id).cloned()
    }

    /// Snapshot of channel metadata.
    pub async fn channel_metas(&self) -> HashMap<String, ChannelMeta> {
        self.conv_maps.read().await.channel_meta.clone()
    }

    /// Snapshot of DM conversations.
    pub async fn dm_conversations(&self) -> HashMap<String, DmPeer> {
        self.conv_maps.read().await.dm_convs.clone()
    }

    /// Get the DM conversation ID for a peer.
    pub async fn dm_conversation_for_peer(&self, peer_id: &str) -> Option<String> {
        self.conv_maps.read().await.dm_by_peer.get(peer_id).cloned()
    }

    /// Queue a raw encoded frame for sending.
    async fn queue_send(&self, data: Vec<u8>) -> Result<(), CommError> {
        let inner = self.inner.read().await;
        let tx = inner.send_tx.as_ref().ok_or(CommError::NotConnected)?;
        tx.send(data)
            .await
            .map_err(|_| CommError::Other("send channel closed".into()))
    }

    /// Join a bot stream (e.g. "chat", "installs", "dm").
    pub async fn join_bot_stream(&self, bot_id: &str, stream: &str) -> Result<(), CommError> {
        {
            let mut maps = self.conv_maps.write().await;
            maps.pending_joins.push(format!("{}:{}", bot_id, stream));
        }
        let payload = serde_json::to_vec(&wire::JoinPayload {
            bot_id: bot_id.to_string(),
            stream: stream.to_string(),
            ..Default::default()
        })
        .map_err(|e| CommError::Other(e.to_string()))?;

        let encoded = frame::encode(
            Header { frame_type: frame::TYPE_JOIN_CONVERSATION, ..Default::default() },
            &payload,
        )
        .map_err(|e| CommError::Other(e.to_string()))?;

        self.queue_send(encoded).await
    }

    /// Join a loop channel.
    pub async fn join_loop_channel(&self, channel_id: &str) -> Result<(), CommError> {
        let payload = serde_json::to_vec(&wire::JoinPayload {
            channel_id: channel_id.to_string(),
            ..Default::default()
        })
        .map_err(|e| CommError::Other(e.to_string()))?;

        let encoded = frame::encode(
            Header { frame_type: frame::TYPE_JOIN_CONVERSATION, ..Default::default() },
            &payload,
        )
        .map_err(|e| CommError::Other(e.to_string()))?;

        self.queue_send(encoded).await
    }

    /// Send a message on a conversation.
    pub async fn send_on_conversation(
        &self,
        conversation_id: &str,
        stream: &str,
        content: serde_json::Value,
    ) -> Result<(), CommError> {
        let payload = serde_json::to_vec(&wire::SendPayload {
            conversation_id: conversation_id.to_string(),
            stream: stream.to_string(),
            content,
        })
        .map_err(|e| CommError::Other(e.to_string()))?;

        let encoded = frame::encode(
            Header {
                frame_type: frame::TYPE_SEND_MESSAGE,
                msg_id: self.ulid_gen.next(),
                ..Default::default()
            },
            &payload,
        )
        .map_err(|e| CommError::Other(e.to_string()))?;

        self.queue_send(encoded).await
    }

    /// Acknowledge messages up to seq in a conversation.
    pub async fn ack(&self, conversation_id: &str, acked_seq: u64) -> Result<(), CommError> {
        let payload = serde_json::to_vec(&wire::AckPayload {
            conversation_id: conversation_id.to_string(),
            acked_seq,
        })
        .map_err(|e| CommError::Other(e.to_string()))?;

        let encoded = frame::encode(
            Header { frame_type: frame::TYPE_ACK, ..Default::default() },
            &payload,
        )
        .map_err(|e| CommError::Other(e.to_string()))?;

        self.queue_send(encoded).await
    }

    /// Send a DM on a conversation.
    pub async fn send_dm(&self, conversation_id: &str, text: &str) -> Result<(), CommError> {
        let content = serde_json::json!({ "text": text });
        self.send_on_conversation(conversation_id, "dm", content).await
    }

    /// Send a chat message.
    pub async fn send_chat(&self, text: &str) -> Result<(), CommError> {
        let bot_id = self.bot_id.read().await.clone();
        let key = format!("{}:chat", bot_id);
        let conv_id = self
            .conversation_for_key(&key)
            .await
            .ok_or_else(|| CommError::Other("chat conversation not joined".into()))?;
        let content = serde_json::json!({ "type": "text", "text": text });
        self.send_on_conversation(&conv_id, "chat", content).await
    }
}

impl Default for NeboLoopPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl CommPlugin for NeboLoopPlugin {
    fn name(&self) -> &str {
        "neboloop"
    }

    fn version(&self) -> &str {
        "4.0.0"
    }

    async fn connect(&self, config: HashMap<String, String>) -> Result<(), CommError> {
        let gateway = config
            .get("gateway")
            .ok_or_else(|| CommError::Other("gateway config required".into()))?
            .clone();
        let bot_id = config
            .get("bot_id")
            .ok_or_else(|| CommError::Other("bot_id config required".into()))?
            .clone();
        let token = config
            .get("token")
            .ok_or_else(|| CommError::Other("token config required".into()))?
            .clone();
        let api_server = config
            .get("api_server")
            .cloned()
            .unwrap_or_else(|| derive_api_url(&gateway));

        // Store bot_id
        *self.bot_id.write().await = bot_id.clone();

        // Create API client
        let api = Arc::new(NeboLoopApi::new(api_server, bot_id.clone(), token.clone()));

        // WebSocket connect
        let (ws_stream, _) = tokio_tungstenite::connect_async(&gateway)
            .await
            .map_err(|e| CommError::Other(format!("ws dial: {}", e)))?;

        let (mut write, mut read) = ws_stream.split();

        // Send CONNECT frame
        let connect_payload = serde_json::to_vec(&wire::ConnectPayload {
            bot_id: Some(bot_id.clone()),
            token: Some(token),
        })
        .map_err(|e| CommError::Other(e.to_string()))?;
        let connect_frame = frame::encode(
            Header { frame_type: frame::TYPE_CONNECT, ..Default::default() },
            &connect_payload,
        )
        .map_err(|e| CommError::Other(e.to_string()))?;

        write
            .send(WsMessage::Binary(connect_frame.into()))
            .await
            .map_err(|e| CommError::Other(format!("send connect: {}", e)))?;

        // Read AUTH response (with 10s timeout)
        let auth_msg = tokio::time::timeout(std::time::Duration::from_secs(10), read.next())
            .await
            .map_err(|_| CommError::Other("auth timeout".into()))?
            .ok_or_else(|| CommError::Other("connection closed during auth".into()))?
            .map_err(|e| CommError::Other(format!("read auth: {}", e)))?;

        let auth_data = match auth_msg {
            WsMessage::Binary(data) => data.to_vec(),
            other => {
                return Err(CommError::Other(format!("unexpected ws message: {:?}", other)));
            }
        };

        let (auth_header, auth_payload) =
            frame::decode(&auth_data).map_err(|e| CommError::Other(format!("decode auth: {}", e)))?;

        if auth_header.frame_type == frame::TYPE_AUTH_FAIL {
            let result: wire::AuthResultPayload =
                serde_json::from_slice(auth_payload).unwrap_or_default();
            return Err(CommError::Other(format!("auth failed: {}", result.reason)));
        }

        if auth_header.frame_type != frame::TYPE_AUTH_OK {
            return Err(CommError::Other(format!(
                "unexpected frame type {}",
                auth_header.frame_type
            )));
        }

        // Parse AUTH_OK to extract rotated token (if present)
        if let Ok(auth_result) = serde_json::from_slice::<wire::AuthResultPayload>(auth_payload) {
            if !auth_result.token.is_empty() {
                *self.rotated_token.write().await = Some(auth_result.token);
            }
        }

        info!(gateway = %gateway, bot_id = %bot_id, "connected to neboloop gateway");

        // Set up send channel + cancellation
        let (send_tx, send_rx) = mpsc::channel::<Vec<u8>>(256);
        let cancel = tokio_util::sync::CancellationToken::new();

        // Reset conversation maps for new connection
        {
            let mut maps = self.conv_maps.write().await;
            *maps = ConvMaps::default();
        }

        {
            let mut inner = self.inner.write().await;
            inner.connected = true;
            inner.send_tx = Some(send_tx);
            inner.cancel = Some(cancel.clone());
            inner.api = Some(api);
        }

        // Clone what the read loop needs
        let handler = self.inner.read().await.handler.clone();

        // Channel for join result updates (read loop → join processor)
        let (join_tx, mut join_rx) = mpsc::channel::<JoinUpdate>(64);

        // Spawn read loop with per-connection dedup window
        let read_handler = handler.clone();
        let read_cancel = cancel.clone();
        let dedup = DedupWindow::new();
        tokio::spawn(async move {
            read_loop(read, read_handler, join_tx, dedup, read_cancel).await;
        });

        // Spawn write loop
        let write_cancel = cancel.clone();
        tokio::spawn(async move {
            write_loop(write, send_rx, write_cancel).await;
        });

        // Spawn join processor — writes to self.conv_maps (shared with query methods)
        let maps_for_task = self.conv_maps.clone();
        let join_cancel = cancel.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(update) = join_rx.recv() => {
                        let mut maps = maps_for_task.write().await;
                        maps.apply(update);
                    }
                    _ = join_cancel.cancelled() => break,
                }
            }
        });

        // Subscribe to default bot streams
        for stream_name in &["dm", "installs", "chat", "account", "voice"] {
            {
                let mut maps = self.conv_maps.write().await;
                maps.pending_joins.push(format!("{}:{}", bot_id, stream_name));
            }
            let payload = serde_json::to_vec(&wire::JoinPayload {
                bot_id: bot_id.clone(),
                stream: stream_name.to_string(),
                ..Default::default()
            })
            .map_err(|e| CommError::Other(e.to_string()))?;
            let encoded = frame::encode(
                Header { frame_type: frame::TYPE_JOIN_CONVERSATION, ..Default::default() },
                &payload,
            )
            .map_err(|e| CommError::Other(e.to_string()))?;
            let inner = self.inner.read().await;
            if let Some(tx) = &inner.send_tx {
                let _ = tx.send(encoded).await;
            }
        }

        Ok(())
    }

    async fn disconnect(&self) -> Result<(), CommError> {
        let mut inner = self.inner.write().await;
        if let Some(cancel) = inner.cancel.take() {
            cancel.cancel();
        }
        inner.connected = false;
        inner.send_tx = None;
        info!("neboloop disconnected");
        Ok(())
    }

    fn is_connected(&self) -> bool {
        // Use try_read to avoid blocking; return false if locked
        self.inner
            .try_read()
            .map(|inner| inner.connected)
            .unwrap_or(false)
    }

    async fn send(&self, msg: CommMessage) -> Result<(), CommError> {
        let inner = self.inner.read().await;
        if !inner.connected {
            return Err(CommError::NotConnected);
        }

        // Find conversation for the topic/target
        let conv_id = if !msg.conversation_id.is_empty() {
            msg.conversation_id.clone()
        } else if !msg.to.is_empty() {
            // Try DM peer lookup from shared conversation maps
            let maps = self.conv_maps.read().await;
            maps.dm_by_peer
                .get(&msg.to)
                .cloned()
                .ok_or_else(|| CommError::Other(format!("no conversation for peer {}", msg.to)))?
        } else {
            return Err(CommError::Other("no conversation_id or recipient".into()));
        };

        let stream = if msg.topic.is_empty() { "dm" } else { &msg.topic };
        let content = serde_json::json!({ "text": msg.content });

        let payload = serde_json::to_vec(&wire::SendPayload {
            conversation_id: conv_id,
            stream: stream.to_string(),
            content,
        })
        .map_err(|e| CommError::Other(e.to_string()))?;

        let encoded = frame::encode(
            Header { frame_type: frame::TYPE_SEND_MESSAGE, ..Default::default() },
            &payload,
        )
        .map_err(|e| CommError::Other(e.to_string()))?;

        let tx = inner.send_tx.as_ref().ok_or(CommError::NotConnected)?;
        tx.send(encoded)
            .await
            .map_err(|_| CommError::Other("send channel closed".into()))
    }

    async fn subscribe(&self, topic: &str) -> Result<(), CommError> {
        // NeboLoop uses JoinBotStream instead of topic subscriptions.
        // If topic matches a stream name, join it.
        let bot_id = self.bot_id.read().await.clone();
        self.join_bot_stream(&bot_id, topic).await
    }

    async fn unsubscribe(&self, _topic: &str) -> Result<(), CommError> {
        // NeboLoop doesn't have explicit unsubscribe for bot streams
        Ok(())
    }

    async fn register(&self, _agent_id: &str, _card: &AgentCard) -> Result<(), CommError> {
        // NeboLoop doesn't use agent card registration — identity is via bot_id/JWT
        Ok(())
    }

    async fn deregister(&self) -> Result<(), CommError> {
        Ok(())
    }

    fn set_message_handler(&self, handler: MessageHandler) {
        // Try to set synchronously; if contended just skip
        if let Ok(mut inner) = self.inner.try_write() {
            inner.handler = Some(handler);
        }
    }
}

// Implement optional query traits via REST API delegation.

#[async_trait::async_trait]
impl crate::LoopChannelLister for NeboLoopPlugin {
    async fn list_loop_channels(&self) -> Result<Vec<LoopChannelInfo>, CommError> {
        let api = self
            .inner
            .read()
            .await
            .api
            .clone()
            .ok_or(CommError::NotConnected)?;

        let channels = api.list_bot_channels().await?;
        Ok(channels
            .into_iter()
            .map(|ch| LoopChannelInfo {
                channel_id: ch.channel_id,
                channel_name: ch.channel_name,
                loop_id: ch.loop_id,
                loop_name: ch.loop_name,
            })
            .collect())
    }
}

#[async_trait::async_trait]
impl crate::LoopLister for NeboLoopPlugin {
    async fn list_loops(&self) -> Result<Vec<LoopInfo>, CommError> {
        let api = self
            .inner
            .read()
            .await
            .api
            .clone()
            .ok_or(CommError::NotConnected)?;

        let loops = api.list_bot_loops().await?;
        Ok(loops
            .into_iter()
            .map(|l| LoopInfo {
                id: l.loop_id,
                name: l.loop_name,
                description: l.description,
            })
            .collect())
    }
}

#[async_trait::async_trait]
impl crate::LoopGetter for NeboLoopPlugin {
    async fn get_loop_info(&self, loop_id: &str) -> Result<LoopInfo, CommError> {
        let api = self
            .inner
            .read()
            .await
            .api
            .clone()
            .ok_or(CommError::NotConnected)?;

        let l = api.get_loop(loop_id).await?;
        Ok(LoopInfo {
            id: l.loop_id,
            name: l.loop_name,
            description: l.description,
        })
    }
}

#[async_trait::async_trait]
impl crate::ChannelMessageLister for NeboLoopPlugin {
    async fn list_channel_messages(
        &self,
        channel_id: &str,
        limit: usize,
    ) -> Result<Vec<ChannelMessageItem>, CommError> {
        let api = self
            .inner
            .read()
            .await
            .api
            .clone()
            .ok_or(CommError::NotConnected)?;

        let msgs = api
            .list_channel_messages(channel_id, Some(limit as i64))
            .await?;
        Ok(msgs
            .into_iter()
            .map(|m| ChannelMessageItem {
                id: m.id,
                from: m.from,
                content: m.content,
                created_at: m.created_at,
                role: m.role,
            })
            .collect())
    }
}

#[async_trait::async_trait]
impl crate::ChannelMemberLister for NeboLoopPlugin {
    async fn list_channel_members(
        &self,
        channel_id: &str,
    ) -> Result<Vec<ChannelMemberItem>, CommError> {
        let api = self
            .inner
            .read()
            .await
            .api
            .clone()
            .ok_or(CommError::NotConnected)?;

        let members = api.list_channel_members(channel_id).await?;
        Ok(members
            .into_iter()
            .map(|m| ChannelMemberItem {
                bot_id: m.bot_id,
                bot_name: m.bot_name,
                role: if m.role.is_empty() { None } else { Some(m.role) },
                is_online: m.is_online,
            })
            .collect())
    }
}

// ── Background tasks ─────────────────────────────────────────────────

/// Join result update sent from read loop to the maps processor.
enum JoinUpdate {
    BotStream { key: String, conversation_id: String },
    Channel(ChannelMeta, String),   // meta, conversation_id
    Dm(DmPeer, String),             // peer, conversation_id
}

/// Shared conversation maps updated by the join processor task.
#[derive(Default)]
struct ConvMaps {
    conv_by_key: HashMap<String, String>,
    pending_joins: Vec<String>,
    channel_convs: HashMap<String, String>,
    channel_by_conv: HashMap<String, String>,
    channel_meta: HashMap<String, ChannelMeta>,
    dm_convs: HashMap<String, DmPeer>,
    dm_by_peer: HashMap<String, String>,
}

impl ConvMaps {
    fn apply(&mut self, update: JoinUpdate) {
        match update {
            JoinUpdate::BotStream { key, conversation_id } => {
                self.conv_by_key.insert(key, conversation_id);
            }
            JoinUpdate::Channel(meta, conv_id) => {
                self.channel_by_conv
                    .insert(conv_id.clone(), meta.channel_id.clone());
                self.channel_convs
                    .insert(meta.channel_id.clone(), conv_id);
                self.channel_meta
                    .insert(meta.channel_id.clone(), meta);
            }
            JoinUpdate::Dm(peer, conv_id) => {
                self.dm_by_peer
                    .insert(peer.peer_id.clone(), conv_id.clone());
                self.dm_convs.insert(conv_id, peer);
            }
        }
    }
}

/// Read loop — receives WebSocket messages, decodes frames, dispatches.
async fn read_loop(
    mut read: SplitStream<WsStream>,
    handler: Option<MessageHandler>,
    join_tx: mpsc::Sender<JoinUpdate>,
    dedup: DedupWindow,
    cancel: tokio_util::sync::CancellationToken,
) {
    loop {
        tokio::select! {
            msg = read.next() => {
                let msg = match msg {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => {
                        warn!(error = %e, "neboloop read error");
                        break;
                    }
                    None => break,
                };

                let data = match msg {
                    WsMessage::Binary(d) => d.to_vec(),
                    WsMessage::Ping(_) | WsMessage::Pong(_) => continue,
                    WsMessage::Close(_) => break,
                    _ => continue,
                };

                let (header, mut payload) = match frame::decode(&data) {
                    Ok(r) => r,
                    Err(e) => {
                        debug!(error = %e, "bad frame");
                        continue;
                    }
                };

                // Decompress if needed
                let decompressed;
                if header.is_compressed() {
                    match compress::decompress(payload) {
                        Ok(d) => {
                            decompressed = d;
                            payload = &decompressed;
                        }
                        Err(e) => {
                            debug!(error = %e, "decompress failed");
                            continue;
                        }
                    }
                }

                match header.frame_type {
                    frame::TYPE_MESSAGE_DELIVERY => {
                        // Skip duplicate messages (same msg_id seen within sliding window)
                        if dedup.is_duplicate(header.msg_id) {
                            debug!("duplicate message, skipping");
                            continue;
                        }

                        let delivery: wire::DeliveryPayload = match serde_json::from_slice(payload) {
                            Ok(d) => d,
                            Err(_) => continue,
                        };

                        let msg = CommMessage {
                            id: uuid_from_bytes(&header.msg_id),
                            from: delivery.sender_id.clone(),
                            to: String::new(),
                            topic: delivery.stream.clone(),
                            conversation_id: uuid_from_bytes(&header.conversation_id),
                            msg_type: CommMessageType::Message,
                            content: delivery.content.to_string(),
                            metadata: HashMap::new(),
                            timestamp: 0,
                            human_injected: false,
                            human_id: None,
                            task_id: None,
                            correlation_id: None,
                            task_status: None,
                            artifacts: vec![],
                            error: None,
                        };

                        if let Some(ref h) = handler {
                            h(msg);
                        }
                    }

                    frame::TYPE_JOIN_CONVERSATION => {
                        let result: wire::JoinResultPayload = match serde_json::from_slice(payload) {
                            Ok(r) => r,
                            Err(_) => continue,
                        };

                        if !result.peer_id.is_empty() {
                            // DM join
                            let _ = join_tx
                                .send(JoinUpdate::Dm(
                                    DmPeer {
                                        peer_id: result.peer_id,
                                        peer_type: result.peer_type,
                                        loop_id: result.loop_id,
                                    },
                                    result.conversation_id,
                                ))
                                .await;
                        } else if !result.channel_id.is_empty() {
                            // Channel join
                            let _ = join_tx
                                .send(JoinUpdate::Channel(
                                    ChannelMeta {
                                        channel_id: result.channel_id,
                                        channel_name: result.channel_name,
                                        loop_id: result.loop_id,
                                    },
                                    result.conversation_id,
                                ))
                                .await;
                        } else {
                            // Bot stream join — use pending_joins queue
                            // We don't have access to the queue here, so send
                            // a generic update and let the processor match it.
                            let _ = join_tx
                                .send(JoinUpdate::BotStream {
                                    key: String::new(), // processor will pop pending
                                    conversation_id: result.conversation_id,
                                })
                                .await;
                        }
                    }

                    frame::TYPE_REPLAY => {
                        debug!("replay frame received");
                    }

                    _ => {
                        debug!(frame_type = header.frame_type, "unhandled frame type");
                    }
                }
            }
            _ = cancel.cancelled() => break,
        }
    }
    debug!("neboloop read loop exited");
}

/// Write loop — sends queued frames and periodic pings.
async fn write_loop(
    mut write: SplitSink<WsStream, WsMessage>,
    mut send_rx: mpsc::Receiver<Vec<u8>>,
    cancel: tokio_util::sync::CancellationToken,
) {
    let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(15));
    ping_interval.tick().await; // skip first immediate tick

    loop {
        tokio::select! {
            Some(data) = send_rx.recv() => {
                if let Err(e) = write.send(WsMessage::Binary(data.into())).await {
                    warn!(error = %e, "neboloop write error");
                    break;
                }
            }
            _ = ping_interval.tick() => {
                if let Err(e) = write.send(WsMessage::Ping(vec![].into())).await {
                    debug!(error = %e, "neboloop ping error");
                    break;
                }
            }
            _ = cancel.cancelled() => break,
        }
    }
    debug!("neboloop write loop exited");
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Format 16 bytes as a UUID string.
fn uuid_from_bytes(b: &[u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15],
    )
}

/// Derive REST API URL from a WebSocket gateway URL.
/// e.g. "wss://comms.neboloop.com/ws" → "https://api.neboloop.com"
fn derive_api_url(gateway: &str) -> String {
    if gateway.contains("localhost") || gateway.contains("127.0.0.1") {
        return "http://localhost:8888".to_string();
    }
    // Production: replace comms subdomain with api
    gateway
        .replace("wss://comms.", "https://api.")
        .replace("ws://comms.", "http://api.")
        .replace("/ws", "")
}
