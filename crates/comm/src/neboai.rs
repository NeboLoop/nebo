//! NeboAI WebSocket plugin — implements `CommPlugin` for the NeboAI comms
//! gateway. Connects via tokio-tungstenite, authenticates with binary framing,
//! and dispatches typed messages (installs, chat, DMs, loop channels, voice).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use tokio::sync::{Notify, RwLock, mpsc};
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, info, warn};

use crate::api::NeboAIApi;
use crate::compress;
use crate::dedup::DedupWindow;
use crate::devlog::DevLog;
use crate::frame::{self, Header};
use crate::ulid::UlidGen;
use crate::wire;
use crate::{
    AgentCard, ChannelMemberItem, ChannelMessageItem, CommError, CommMessage, CommMessageType,
    CommPlugin, LoopChannelInfo, LoopInfo, MessageHandler, Outbox, StreamOffsets,
};

type WsStream = tokio_tungstenite::WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// The bot's own hub streams, JOINed on every connect with their acked
/// offsets. `channels/inbound` carries inbound email. The hub subscribes the
/// bot to `installs`, `tasks`, `card` and `apps` itself and backfills those
/// from its own record of our acks.
const BOT_STREAMS: &[&str] = &[
    "dm",
    "installs",
    "chat",
    "account",
    "voice",
    "channels/inbound",
];

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

/// Agent space metadata tracked after JOIN responses.
#[derive(Debug, Clone)]
pub(crate) struct AgentSpaceMeta {
    pub agent_id: String,
    pub agent_slug: String,
    pub loop_id: String,
    /// Desktop chat this conversation is bound to ('general' = the legacy
    /// single conversation; empty from servers that predate per-chat spaces).
    pub chat_id: String,
    pub chat_title: String,
}

struct Inner {
    send_tx: Option<mpsc::Sender<Vec<u8>>>,
    cancel: Option<tokio_util::sync::CancellationToken>,
    api: Option<Arc<NeboAIApi>>,
}

/// NeboAI WebSocket CommPlugin.
pub struct NeboAIPlugin {
    inner: RwLock<Inner>,
    /// Message handler — separate sync lock so `set_message_handler()` never fails.
    /// Using `std::sync::RwLock` (not tokio) because the trait method is sync.
    handler: std::sync::RwLock<Option<MessageHandler>>,
    bot_id: RwLock<String>,
    /// Rotated bot JWT from the last AUTH_OK (token rotation).
    rotated_token: RwLock<Option<String>>,
    /// Conversation maps — updated by the join processor, queried by public methods.
    conv_maps: Arc<RwLock<ConvMaps>>,
    /// Monotonic ULID generator for outgoing message IDs.
    ulid_gen: UlidGen,
    /// Dev log for `tail -f` traffic inspection (set during connect).
    devlog: RwLock<Option<DevLog>>,
    /// Lock-free connected flag — set true after AUTH_OK, false on disconnect/read-loop exit.
    connected: Arc<AtomicBool>,
    /// Signalled when the read loop exits unexpectedly (not via cancel).
    disconnect_notify: Arc<Notify>,
    /// How the last connection ended (a drain, a hard cut, another close),
    /// set by the read loop and taken by `wait_disconnect`.
    ended: Arc<std::sync::Mutex<Option<crate::reconnect::Disconnect>>>,
    /// Durable acked offsets of the bot's own streams.
    offsets: Arc<dyn StreamOffsets>,
    /// Set by the read loop when the hub closed the connection naming the
    /// cell the account lives in; the next connect dials there first.
    redirect_to: Arc<std::sync::Mutex<Option<crate::cell::Redirect>>>,
    /// Where every durable outbound message waits until it is sent
    /// (`with_outbox`); none sends straight through.
    outbox: Option<Arc<dyn Outbox>>,
    /// One pass over the outbox at a time, so messages leave in order.
    flushing: tokio::sync::Mutex<()>,
    /// The current connection's AUTH_OK (unix ms); 0 before the first.
    connected_since: Arc<AtomicI64>,
    /// The current connection's hub dedupes a repeated msg id (AUTH_OK
    /// feature [`MSG_DEDUP`]), so a send that may have reached it can go
    /// again.
    hub_dedupes: AtomicBool,
}

/// The AUTH_OK feature saying the hub stores and delivers a SEND that
/// repeats an earlier one's msg id only once.
pub const MSG_DEDUP: &str = "msg_dedup";

/// A reconnect's backlog goes out at the hub's sustained send rate (one a
/// second) once this many have used up the connection's burst allowance
/// (the gateway's is 20, shared with the bot's other sends). The hub drops
/// what goes over the limit without a word to a bot.
const FLUSH_BURST: usize = 10;

/// With a deduping hub, a handed-off message is kept until its connection
/// has stayed up this long after (the reconnect policy's healthy session),
/// and sent again if that connection drops first. The hub confirms nothing,
/// so a connection that lived on is the confirmation.
const CONFIRMED_AFTER_MS: i64 = crate::reconnect::HEALTHY.as_millis() as i64;

fn unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl NeboAIPlugin {
    pub fn new(offsets: Arc<dyn StreamOffsets>) -> Self {
        Self {
            inner: RwLock::new(Inner {
                send_tx: None,
                cancel: None,
                api: None,
            }),
            handler: std::sync::RwLock::new(None),
            bot_id: RwLock::new(String::new()),
            rotated_token: RwLock::new(None),
            conv_maps: Arc::new(RwLock::new(ConvMaps::default())),
            ulid_gen: UlidGen::new(),
            devlog: RwLock::new(None),
            connected: Arc::new(AtomicBool::new(false)),
            disconnect_notify: Arc::new(Notify::new()),
            ended: Arc::new(std::sync::Mutex::new(None)),
            offsets,
            redirect_to: Arc::new(std::sync::Mutex::new(None)),
            outbox: None,
            flushing: tokio::sync::Mutex::new(()),
            connected_since: Arc::new(AtomicI64::new(0)),
            hub_dedupes: AtomicBool::new(false),
        }
    }

    /// Keep every durable outbound message in `outbox` until it is sent.
    pub fn with_outbox(mut self, outbox: Arc<dyn Outbox>) -> Self {
        self.outbox = Some(outbox);
        self
    }

    /// One pass over the outbox, oldest first. Each message that never left
    /// is handed to the connection; one that may have left on an earlier
    /// connection (or process) goes again only where the hub dedupes by msg
    /// id, and is otherwise dropped, since a second copy would be stored and
    /// shown twice. The pass stops at the first message that has to wait for
    /// a connection, so a conversation's messages never pass each other.
    /// Returns the messages refused for good in this pass, by id.
    async fn flush(&self) -> HashMap<String, CommError> {
        let mut refused = HashMap::new();
        let Some(ref outbox) = self.outbox else {
            return refused;
        };
        let _one_pass = self.flushing.lock().await;
        // Nothing goes without a connection, and whether its hub dedupes is
        // not known before one.
        if !self.connected.load(Ordering::SeqCst) {
            return refused;
        }
        let dedupes = self.hub_dedupes.load(Ordering::SeqCst);
        let since = self.connected_since.load(Ordering::SeqCst);
        let mut sent = 0;
        for (msg, handed_off) in outbox.pending() {
            let id = msg.id.clone();
            match handed_off {
                // Out on this connection, waiting for it to live on.
                Some(at) if since > 0 && at >= since => {
                    if unix_ms() - at >= CONFIRMED_AFTER_MS {
                        self.confirmed(&id);
                    }
                    continue;
                }
                Some(_) if !dedupes => {
                    warn!(msg_id = %id, "comm message may have reached the hub before the connection dropped; not sent again");
                    outbox.remove(&id);
                    continue;
                }
                _ => {}
            }
            if sent >= FLUSH_BURST {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            // The same wire id on every send of the message: what a
            // deduping hub recognizes a second copy by.
            let msg_id = uuid::Uuid::parse_str(&id)
                .map(|u| *u.as_bytes())
                .unwrap_or_else(|_| self.ulid_gen.next());
            let frame = match self.encode_send(&msg, msg_id).await {
                Ok(frame) => frame,
                Err(CommError::NotConnected | CommError::Paused | CommError::NoActivePlugin) => break,
                Err(e) => {
                    warn!(msg_id = %id, error = %e, "comm message refused for good; dropped");
                    outbox.remove(&id);
                    refused.insert(id, e);
                    continue;
                }
            };
            // From here its bytes may leave.
            outbox.handed_off(&id, Some(unix_ms()));
            if self.queue_send(frame).await.is_err() {
                // The connection went before the frame was queued: never left.
                outbox.handed_off(&id, None);
                break;
            }
            sent += 1;
            if !dedupes {
                // Never sent again, so nothing to keep.
                self.confirmed(&id);
            }
        }
        refused
    }

    /// The one place a handed-off message counts as delivered and leaves
    /// the outbox. Today that is the heuristic in `flush` (its connection
    /// lived on for [`CONFIRMED_AFTER_MS`], or the hub cannot dedupe a
    /// resend anyway); a hub acknowledging each SEND would call this from
    /// the read loop instead.
    fn confirmed(&self, id: &str) {
        if let Some(ref outbox) = self.outbox {
            outbox.remove(id);
        }
    }

    /// Encode one message as a SEND frame for the current connection.
    /// Refused with `NotConnected` while there is none and `Paused` while
    /// this process's lease is not held (another process may be the bot
    /// now); any other refusal means it can never be sent.
    async fn encode_send(&self, msg: &CommMessage, msg_id: [u8; 16]) -> Result<Vec<u8>, CommError> {
        if crate::lease::process().frozen() {
            return Err(CommError::Paused);
        }
        if !self.connected.load(Ordering::SeqCst) {
            return Err(CommError::NotConnected);
        }

        // LoopChannel sends carry the CHANNEL ID (the loop tool has no access to
        // the channel→conversation map). But the gateway routes channel messages
        // by the channel's distinct conversation_id, on the fixed "channel" stream.
        // Resolve here — without this the SendPayload targets a conversation that
        // doesn't exist and the message is silently dropped (the send reports
        // success because it's fire-and-forget). Done before taking `inner` so the
        // join-on-miss path doesn't deadlock on the lock.
        let loop_channel_conv = if matches!(msg.msg_type, CommMessageType::LoopChannel) {
            let channel_id = if !msg.conversation_id.is_empty() {
                msg.conversation_id.clone()
            } else {
                msg.topic.clone()
            };
            Some(self.resolve_channel_conv(&channel_id).await?)
        } else {
            None
        };

        // Find conversation for the topic/target
        let conv_id = if let Some(ref c) = loop_channel_conv {
            c.clone()
        } else if !msg.conversation_id.is_empty() {
            msg.conversation_id.clone()
        } else if !msg.to.is_empty() {
            // Try agent space first, then DM peer lookup. Right after a
            // connect the maps are still filling from JOIN results (a queued
            // message goes out then), so a miss waits briefly, as a channel's
            // does in `resolve_channel_conv`.
            let mut found = None;
            for _ in 0..30 {
                let maps = self.conv_maps.read().await;
                found = maps
                    .agent_space_by_slug
                    .get(&msg.to)
                    .or_else(|| maps.dm_by_peer.get(&msg.to))
                    .cloned();
                drop(maps);
                if found.is_some() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            found.ok_or_else(|| CommError::Other(format!("no conversation for {}", msg.to)))?
        } else {
            return Err(CommError::Other("no conversation_id or recipient".into()));
        };

        let stream = if loop_channel_conv.is_some() {
            // Channel messages always go out on the fixed "channel" stream
            // (mirrors the agent's channel-reply path), not the channel UUID.
            "channel"
        } else if msg.topic.is_empty() {
            // Default to agent_space if we resolved via agent slug, else dm
            let maps = self.conv_maps.read().await;
            if !msg.to.is_empty() && maps.agent_space_by_slug.contains_key(&msg.to) {
                "agent_space"
            } else {
                "dm"
            }
        } else if msg.topic == "embed" {
            // Embed replies go out on the "chat" stream — the web widget listens
            // for non-ephemeral streams and merges `type: "stream"` chunks,
            // finalizing on the closing typeless message (like agent space).
            "chat"
        } else {
            &msg.topic
        };
        let mut content = serde_json::json!({ "text": msg.content });
        // Streaming chunks AND tool-activity are ephemeral — transient signals
        // the frontend assembles live, not persisted/backfilled as bubbles.
        let ephemeral = matches!(
            msg.msg_type,
            CommMessageType::Stream | CommMessageType::ToolActivity
        );
        // Tag the message type so the frontend can route it: streaming chunks
        // (`type: "stream"`) accumulate into one bubble; `tool_activity`
        // collapses into the "Used N tools" timeline; etc. The closing
        // CommMessageType::Message stays TYPELESS so the web finalizes
        // (replaces) the accumulated streaming bubble with the full text.
        if !matches!(msg.msg_type, CommMessageType::Message) {
            if let Ok(serde_json::Value::String(t)) = serde_json::to_value(&msg.msg_type) {
                content["type"] = serde_json::Value::String(t);
            }
        }
        // Metadata rides nested under content.metadata — the canonical place
        // the web reads (attachToolActivity, suggestions, ask options, stop
        // filters all read content.metadata.*). Root-level copies remain for
        // older web fallback readers (content.senderName); drop them once no
        // deployed frontend reads root fields.
        if !msg.metadata.is_empty() {
            content["metadata"] = serde_json::to_value(&msg.metadata).unwrap_or_default();
        }
        if let serde_json::Value::Object(ref mut map) = content {
            for (k, v) in &msg.metadata {
                if k != "metadata" {
                    map.insert(k.clone(), serde_json::Value::String(v.clone()));
                }
            }
        }
        // Include file/image/video attachments if present
        if !msg.attachments.is_empty() {
            content["attachments"] = serde_json::to_value(&msg.attachments)
                .unwrap_or_default();
        }
        // The message's own id rides every send of it, a resend from the
        // outbox included, so a receiver can tell a second copy from a new
        // message: the hub stamps each send with a fresh id of its own. Set
        // after the metadata copies so a lifted `clientId` never replaces it.
        if !ephemeral && !msg.id.is_empty() {
            content["clientId"] = serde_json::Value::String(msg.id.clone());
        }

        if let Some(ref dl) = *self.devlog.read().await {
            dl.outbound(stream, &conv_id, &msg.content);
        }

        // Instrument the RESPONSE: what identity we attach to a reply. The
        // sending agent rides the wire as fromAgentId/fromAgentName (stamped
        // into msg.metadata by the dispatcher); senderName in content remains
        // for older web readers.
        tracing::info!(
            target: "neboai_identity",
            conv_id = %conv_id,
            stream = %stream,
            msg_type = ?msg.msg_type,
            to = %msg.to,
            from = %msg.from,
            sender_name = ?msg.metadata.get("senderName"),
            meta_keys = ?msg.metadata.keys().collect::<Vec<_>>(),
            "OUTBOUND send() — what identity we attach to the response"
        );

        let payload = serde_json::to_vec(&wire::SendPayload {
            conversation_id: conv_id,
            stream: stream.to_string(),
            content,
            from_agent_id: msg
                .metadata
                .get("fromAgentId")
                .cloned()
                .unwrap_or_default(),
            from_agent_name: msg
                .metadata
                .get("fromAgentName")
                .cloned()
                .unwrap_or_default(),
        })
        .map_err(|e| CommError::Other(e.to_string()))?;

        // Stream chunks are ephemeral (fanout-only, not persisted): only the
        // final Message is durable, so history replay shows one clean message
        // rather than the intermediate fragments.
        let flags = if ephemeral { frame::FLAG_EPHEMERAL } else { 0 };
        frame::encode(
            Header {
                frame_type: frame::TYPE_SEND_MESSAGE,
                flags,
                msg_id,
                ..Default::default()
            },
            &payload,
        )
        .map_err(|e| CommError::Other(e.to_string()))
    }

    /// Returns the rotated bot JWT from the last AUTH_OK, if any.
    /// The caller should persist this token and use it for the next connect.
    pub async fn take_rotated_token(&self) -> Option<String> {
        self.rotated_token.write().await.take()
    }

    /// Get the API client (available after connect).
    pub async fn api(&self) -> Option<Arc<NeboAIApi>> {
        self.inner.read().await.api.clone()
    }

    /// Get the conversation ID for a given key (e.g. "botId:chat").
    pub async fn conversation_for_key(&self, key: &str) -> Option<String> {
        self.conv_maps.read().await.conv_by_key.get(key).cloned()
    }

    /// Get the conversation ID for a channel.
    pub async fn conversation_for_channel(&self, channel_id: &str) -> Option<String> {
        self.conv_maps
            .read()
            .await
            .channel_convs
            .get(channel_id)
            .cloned()
    }

    /// Resolve a channel's ID to its routing conversation_id for an outbound send.
    /// If the channel isn't joined yet, join it and wait briefly for the JOIN
    /// result to populate `channel_convs` (it lands asynchronously via the inbound
    /// handler). Errors if it never resolves so the caller doesn't report a false
    /// success for an undeliverable message.
    async fn resolve_channel_conv(&self, channel_id: &str) -> Result<String, CommError> {
        if let Some(c) = self.conversation_for_channel(channel_id).await {
            return Ok(c);
        }
        self.join_loop_channel(channel_id).await?;
        for _ in 0..30 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            if let Some(c) = self.conversation_for_channel(channel_id).await {
                return Ok(c);
            }
        }
        Err(CommError::Other(format!(
            "channel {channel_id} could not be resolved (bot not a member?) — message not sent"
        )))
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

    /// Get the agent space conversation ID for an agent slug.
    pub async fn agent_space_for_slug(&self, slug: &str) -> Option<String> {
        self.conv_maps
            .read()
            .await
            .agent_space_by_slug
            .get(slug)
            .cloned()
    }


    /// Queue a raw encoded frame for sending.
    async fn queue_send(&self, data: Vec<u8>) -> Result<(), CommError> {
        let inner = self.inner.read().await;
        let tx = inner.send_tx.as_ref().ok_or(CommError::NotConnected)?;
        tx.send(data)
            .await
            .map_err(|_| CommError::NotConnected)
    }

    /// Join a bot stream (e.g. "chat", "installs", "dm") from its last acked
    /// seq, so the hub replays whatever arrived while we were disconnected.
    pub async fn join_bot_stream(&self, bot_id: &str, stream: &str) -> Result<(), CommError> {
        if let Some(ref dl) = *self.devlog.read().await {
            dl.join_request(stream);
        }
        let payload = serde_json::to_vec(&wire::JoinPayload {
            bot_id: bot_id.to_string(),
            stream: stream.to_string(),
            last_acked_seq: self.offsets.acked(bot_id, stream),
            ..Default::default()
        })
        .map_err(|e| CommError::Other(e.to_string()))?;

        let encoded = frame::encode(
            Header {
                frame_type: frame::TYPE_JOIN_CONVERSATION,
                ..Default::default()
            },
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
            Header {
                frame_type: frame::TYPE_JOIN_CONVERSATION,
                ..Default::default()
            },
            &payload,
        )
        .map_err(|e| CommError::Other(e.to_string()))?;

        self.queue_send(encoded).await
    }

    /// Send a message on a conversation. When `ephemeral` is true the SEND
    /// frame carries `FLAG_EPHEMERAL`, so the gateway fans it out without
    /// persisting or deduping it (used for transient signals like typing).
    pub async fn send_on_conversation(
        &self,
        conversation_id: &str,
        stream: &str,
        content: serde_json::Value,
        ephemeral: bool,
    ) -> Result<(), CommError> {
        let payload = serde_json::to_vec(&wire::SendPayload {
            conversation_id: conversation_id.to_string(),
            stream: stream.to_string(),
            content,
            // Main-bot surface sends (dm/chat/typing) carry no agent identity.
            from_agent_id: String::new(),
            from_agent_name: String::new(),
        })
        .map_err(|e| CommError::Other(e.to_string()))?;

        let flags = if ephemeral { frame::FLAG_EPHEMERAL } else { 0 };
        let encoded = frame::encode(
            Header {
                frame_type: frame::TYPE_SEND_MESSAGE,
                flags,
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
        self.queue_send(encode_ack(conversation_id, acked_seq)?).await
    }

    /// Send a DM on a conversation.
    pub async fn send_dm(&self, conversation_id: &str, text: &str) -> Result<(), CommError> {
        let content = serde_json::json!({ "text": text });
        self.send_on_conversation(conversation_id, "dm", content, false)
            .await
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
        self.send_on_conversation(&conv_id, "chat", content, false)
            .await
    }
}

#[async_trait::async_trait]
impl CommPlugin for NeboAIPlugin {
    fn name(&self) -> &str {
        "neboai"
    }

    fn version(&self) -> &str {
        "4.0.0"
    }

    async fn connect(&self, config: HashMap<String, String>) -> Result<(), CommError> {
        // Tear down existing connection first to avoid ghost connections.
        // Without this, the old read/write loops keep running (the write loop
        // continues sending pings even after its send channel closes), creating
        // duplicate WebSocket connections to the gateway.
        if self.is_connected() {
            self.disconnect().await.ok();
        } else {
            // Even if is_connected() returns false, clean up any stale state
            // (e.g., cancelled token, dead send channel) so the new connection
            // starts fresh.
            let mut inner = self.inner.write().await;
            if let Some(cancel) = inner.cancel.take() {
                cancel.cancel();
            }
            inner.send_tx = None;
            self.connected.store(false, Ordering::SeqCst);
        }

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

        // Init devlog (appends with session separator) — dev builds only
        #[cfg(debug_assertions)]
        if let Some(dir) = config.get("data_dir") {
            let log_path = std::path::PathBuf::from(dir)
                .join("logs")
                .join("neboai.log");
            if let Some(dl) = DevLog::open(&log_path) {
                dl.event("────────────────────────────────────────");
                dl.event(&format!("CONNECT {} bot={}", gateway, &bot_id));
                *self.devlog.write().await = Some(dl);
            }
        }

        // Create API client
        let api = Arc::new(NeboAIApi::new(api_server, bot_id.clone(), token.clone()));

        // This process is about to act as the bot: frozen (where freezing is
        // on) until the hub grants the lease on AUTH_OK.
        let lease = crate::lease::process();
        lease.claim();

        // Send CONNECT frame — carries the bot's configured identity so the
        // loop agent reflects the local Identity settings. Identity fields are
        // plumbed in via the same config map as bot_id/token (see
        // codes::activate_neboai). Empty values are dropped (None).
        let connect_identity = wire::ConnectPayload {
            bot_id: Some(bot_id.clone()),
            token: Some(token),
            agent_name: config.get("agent_name").filter(|v| !v.is_empty()).cloned(),
            agent_handle: config
                .get("agent_handle")
                .filter(|v| !v.is_empty())
                .cloned(),
            agent_color: config
                .get("agent_color")
                .filter(|v| !v.is_empty())
                .cloned(),
            platform: config.get("platform").filter(|v| !v.is_empty()).cloned(),
            hostname: config.get("hostname").filter(|v| !v.is_empty()).cloned(),
            runtime: config.get("runtime").filter(|v| !v.is_empty()).cloned(),
            chat: config.get("chat").is_some_and(|v| v == "true"),
            // The read loop acks every delivery it dispatches, so the gateway
            // may safely backfill this connection's agent spaces.
            acks_offsets: true,
            // Ask for the bot's lease (crate::lease).
            instance_id: Some(lease.instance_id().to_string()),
            lease_epoch: lease.epoch(),
        };
        tracing::info!(
            target: "neboai_identity",
            bot_id = %bot_id,
            agent_name = ?connect_identity.agent_name,
            agent_handle = ?connect_identity.agent_handle,
            agent_color = ?connect_identity.agent_color,
            "CONNECT: announcing primary identity to loop"
        );
        let connect_payload =
            serde_json::to_vec(&connect_identity).map_err(|e| CommError::Other(e.to_string()))?;
        let connect_frame = frame::encode(
            Header {
                frame_type: frame::TYPE_CONNECT,
                ..Default::default()
            },
            &connect_payload,
        )
        .map_err(|e| CommError::Other(e.to_string()))?;

        // Dial and authenticate. A cell the account does not live in names
        // the one it does: dial there within half a second, quietly, at most
        // twice in a row (crate::cell). A close naming another cell on the
        // last connection sends the first dial there.
        let mut redirects = crate::cell::Redirects::new();
        let moved = self.redirect_to.lock().unwrap_or_else(|e| e.into_inner()).take();
        let mut dial_url = moved
            .and_then(|r| redirects.follow(&gateway, &r))
            .unwrap_or_else(|| gateway.clone());
        let (write, read, connect_sent, auth_data) = loop {
            let redirect = match dial_and_auth(&dial_url, &connect_frame).await? {
                Auth::Answered { write, read, connect_sent, data } => {
                    break (write, read, connect_sent, data);
                }
                Auth::Redirected(redirect) => redirect,
            };
            if let Some(ref dl) = *self.devlog.read().await {
                dl.event(&format!("REDIRECT cell={} url={}", redirect.cell, redirect.url));
            }
            let Some(next) = redirects.follow(&gateway, &redirect) else {
                return Err(CommError::Other(format!(
                    "not following the hub to cell {}",
                    redirect.cell
                )));
            };
            tokio::time::sleep(crate::cell::jitter()).await;
            dial_url = next;
        };

        let (auth_header, auth_payload) = frame::decode(&auth_data)
            .map_err(|e| CommError::Other(format!("decode auth: {}", e)))?;

        if auth_header.frame_type == frame::TYPE_AUTH_FAIL {
            let result: wire::AuthResultPayload =
                serde_json::from_slice(auth_payload).unwrap_or_default();
            if let Some(ref dl) = *self.devlog.read().await {
                dl.error(&format!("AUTH_FAIL: {}", result.reason));
            }
            if result.reason == "lease_held" {
                warn!(bot_id = %bot_id, instance = %lease.instance_id(), "neboai: another running copy of this bot holds its lease; this process stays off");
                lease.lost();
                return Err(CommError::LeaseHeld);
            }
            if result.reason == crate::REVOKED_REASON {
                warn!(bot_id = %bot_id, "neboai: this bot was removed from NeboAI");
                return Err(CommError::Revoked);
            }
            return Err(CommError::AuthFailed(result.reason));
        }

        if auth_header.frame_type != frame::TYPE_AUTH_OK {
            if let Some(ref dl) = *self.devlog.read().await {
                dl.error(&format!(
                    "unexpected frame type {} during auth",
                    auth_header.frame_type
                ));
            }
            return Err(CommError::Other(format!(
                "unexpected frame type {}",
                auth_header.frame_type
            )));
        }

        // Parse AUTH_OK to extract rotated token (if present)
        let auth_result = serde_json::from_slice::<wire::AuthResultPayload>(auth_payload);
        self.hub_dedupes.store(
            auth_result
                .as_ref()
                .is_ok_and(|r| r.features.iter().any(|f| f == MSG_DEDUP)),
            Ordering::SeqCst,
        );
        if let Ok(auth_result) = auth_result {
            // The lease this process now holds; epoch 0 = a hub that issues
            // none. An unreadable AUTH_OK leaves the lease unconfirmed.
            let ttl = match auth_result.lease_ttl_secs {
                0 => std::time::Duration::from_secs(60),
                secs => std::time::Duration::from_secs(secs),
            };
            if !auth_result.token.is_empty() {
                *self.rotated_token.write().await = Some(auth_result.token.clone());

                // The gateway invalidated the connect token the moment it rotated
                // (token_issued_at check in the loop's REST middleware), so the
                // REST client must switch to the rotated token NOW — not on next
                // boot — or every channel/group API call 401s with "stale token".
                api.set_token(auth_result.token.clone());

                // Persist rotated token to cache file immediately — if the process
                // is killed (e.g. hot reload) before the caller can persist to DB,
                // the next startup reads this file and avoids "stale token" failure.
                if let Some(dir) = config.get("data_dir") {
                    let cache_path = std::path::PathBuf::from(dir).join("neboai_token.cache");
                    if let Err(e) = std::fs::write(&cache_path, &auth_result.token) {
                        warn!(error = %e, "failed to cache rotated token");
                    }
                }
            }
            // Granted last: the grant wakes the hub's other doors (the
            // tunnel dial), and the connect token is dead the moment the hub
            // rotates it. By now every reader sees the rotated one.
            lease.granted(auth_result.lease_epoch, ttl, connect_sent);
            info!(bot_id = %bot_id, epoch = auth_result.lease_epoch, instance = %lease.instance_id(), "neboai: lease granted");
        }

        if let Some(ref dl) = *self.devlog.read().await {
            dl.event("AUTH_OK");
        }

        info!(gateway = %gateway, bot_id = %bot_id, "connected to neboai gateway");

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
            inner.send_tx = Some(send_tx.clone());
            inner.cancel = Some(cancel.clone());
            inner.api = Some(api);
        }
        self.connected_since.store(unix_ms(), Ordering::SeqCst);
        self.connected.store(true, Ordering::SeqCst);

        // Clone what the read loop needs (handler is in separate sync lock)
        let handler = self.handler.read().unwrap().clone();

        // Channel for join result updates (read loop → join processor)
        let (join_tx, mut join_rx) = mpsc::channel::<JoinUpdate>(64);

        // Spawn read loop with per-connection dedup window
        let read_handler = handler.clone();
        let read_cancel = cancel.clone();
        let dedup = DedupWindow::new();
        let devlog_for_read = self.devlog.read().await.clone();
        let connected_for_read = self.connected.clone();
        let notify_for_read = self.disconnect_notify.clone();
        // How an earlier connection ended says nothing about this one.
        *self.ended.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let ended_for_read = self.ended.clone();
        let redirect_for_read = self.redirect_to.clone();
        let maps_for_read = self.conv_maps.clone();
        let stream_acks = StreamAcks {
            bot_id: bot_id.clone(),
            offsets: self.offsets.clone(),
            stream_by_conv: HashMap::new(),
        };
        tokio::spawn(async move {
            read_loop(
                read,
                read_handler,
                join_tx,
                maps_for_read,
                dedup,
                read_cancel,
                devlog_for_read,
                connected_for_read,
                notify_for_read,
                ended_for_read,
                redirect_for_read,
                send_tx,
                stream_acks,
            )
            .await;
        });

        // Spawn write loop
        let write_cancel = cancel.clone();
        let devlog_for_write = self.devlog.read().await.clone();
        tokio::spawn(async move {
            write_loop(write, send_rx, write_cancel, devlog_for_write).await;
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

        // Join the bot's own streams, each from its last acked seq.
        for stream_name in BOT_STREAMS {
            if let Err(e) = self.join_bot_stream(&bot_id, stream_name).await {
                warn!(stream = %stream_name, error = %e, "bot stream join not queued");
            }
        }

        Ok(())
    }

    async fn disconnect(&self) -> Result<(), CommError> {
        if let Some(ref dl) = *self.devlog.read().await {
            dl.event("DISCONNECT");
        }
        self.connected.store(false, Ordering::SeqCst);
        let mut inner = self.inner.write().await;
        if let Some(cancel) = inner.cancel.take() {
            cancel.cancel();
        }
        inner.send_tx = None;
        info!("neboai disconnected");
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    async fn upload_file(
        &self,
        filename: &str,
        mime_type: &str,
        data: Vec<u8>,
        fields: &[(String, String)],
    ) -> Result<crate::wire::Attachment, CommError> {
        let api = self
            .api()
            .await
            .ok_or(CommError::NotConnected)?;
        api.upload_file(filename, mime_type, data, fields).await
    }

    async fn send(&self, mut msg: CommMessage) -> Result<(), CommError> {
        // Streaming chunks and tool activity are transient signals: one that
        // missed its moment is worth nothing later.
        let transient = matches!(
            msg.msg_type,
            CommMessageType::Stream | CommMessageType::ToolActivity
        );
        let Some(outbox) = self.outbox.as_ref().filter(|_| !transient) else {
            let frame = self.encode_send(&msg, self.ulid_gen.next()).await?;
            return self.queue_send(frame).await;
        };
        // The message's id from here on, on the wire too: a monotonic one,
        // the same on every send of it.
        let msg_id = self.ulid_gen.next();
        msg.id = uuid_from_bytes(&msg_id);
        if !outbox.put(&msg) {
            let frame = self.encode_send(&msg, msg_id).await?;
            return self.queue_send(frame).await;
        }
        let id = msg.id.clone();
        // Queued behind anything older; Ok once sent or kept for the next
        // connection, an error only when it was refused for good.
        match self.flush().await.remove(&id) {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    async fn send_pending(&self) {
        self.flush().await;
    }

    async fn subscribe(&self, topic: &str) -> Result<(), CommError> {
        // NeboAI uses JoinBotStream instead of topic subscriptions.
        // If topic matches a stream name, join it.
        let bot_id = self.bot_id.read().await.clone();
        self.join_bot_stream(&bot_id, topic).await
    }

    async fn unsubscribe(&self, _topic: &str) -> Result<(), CommError> {
        // NeboAI doesn't have explicit unsubscribe for bot streams
        Ok(())
    }

    async fn register(&self, _agent_id: &str, _card: &AgentCard) -> Result<(), CommError> {
        // NeboAI doesn't use agent card registration — identity is via bot_id/JWT
        Ok(())
    }

    async fn deregister(&self) -> Result<(), CommError> {
        Ok(())
    }

    fn set_message_handler(&self, handler: MessageHandler) {
        // Uses std::sync::RwLock (not tokio) — always succeeds, never skipped.
        *self.handler.write().unwrap() = Some(handler);
    }

    async fn list_channels(&self) -> Result<Vec<LoopChannelInfo>, CommError> {
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

    async fn ensure_channel(
        &self,
        name: &str,
        description: Option<&str>,
    ) -> Result<String, CommError> {
        let api = self
            .inner
            .read()
            .await
            .api
            .clone()
            .ok_or(CommError::NotConnected)?;

        let want = sanitize_channel_name(name);

        // Idempotent: reuse an existing channel with the same (sanitized) name.
        let channels = api.list_bot_channels().await?;
        if let Some(ch) = channels
            .iter()
            .find(|c| sanitize_channel_name(&c.channel_name) == want)
        {
            return Ok(ch.channel_id.clone());
        }

        // Create it in the bot's personal (first) loop — NeboLoop auto-adds the
        // bot as a member, so it can post right after.
        let loops = api.list_bot_loops().await?;
        let loop_id = loops
            .first()
            .map(|l| l.loop_id.clone())
            .ok_or_else(|| {
                CommError::Other("no loop available to create a channel in".to_string())
            })?;
        api.create_channel(&loop_id, name, description).await?;

        // Re-list to return the canonical channel_id the send path uses.
        let channels = api.list_bot_channels().await?;
        channels
            .iter()
            .find(|c| sanitize_channel_name(&c.channel_name) == want)
            .map(|c| c.channel_id.clone())
            .ok_or_else(|| {
                CommError::Other("channel created but not found in listing".to_string())
            })
    }

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

    async fn bot_id(&self) -> String {
        self.bot_id.read().await.clone()
    }

    async fn conversation_for_channel(&self, channel_id: &str) -> Option<String> {
        self.resolve_channel_conv(channel_id).await.ok()
    }

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
                sender_name: m.sender_name,
                attachments: m.attachments,
            })
            .collect())
    }

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
                role: if m.role.is_empty() {
                    None
                } else {
                    Some(m.role)
                },
                is_online: m.is_online,
            })
            .collect())
    }

    async fn list_loop_agents(
        &self,
        loop_id: &str,
    ) -> Result<Vec<crate::types::LoopAgentInfo>, CommError> {
        let api = self
            .inner
            .read()
            .await
            .api
            .clone()
            .ok_or(CommError::NotConnected)?;

        let agents = api.list_agents(loop_id, false).await?;
        Ok(agents
            .into_iter()
            .map(|a| crate::types::LoopAgentInfo {
                id: a.id,
                bot_id: a.bot_id,
                name: a.name,
                slug: a.slug,
                bot_name: a.bot_name,
                bot_slug: a.bot_slug,
            })
            .collect())
    }

    async fn take_rotated_token(&self) -> Option<String> {
        self.rotated_token.write().await.take()
    }

    async fn chat_for_conv(&self, conv_id: &str) -> Option<(String, String)> {
        let maps = self.conv_maps.read().await;
        maps.agent_space_convs
            .get(conv_id)
            .map(|m| (m.chat_id.clone(), m.chat_title.clone()))
    }

    async fn agent_chat_conv_for_slug(&self, slug: &str, chat_id: &str) -> Option<String> {
        let maps = self.conv_maps.read().await;
        if chat_id.is_empty() || chat_id == "general" {
            return maps.agent_space_by_slug.get(slug).cloned();
        }
        maps.agent_chat_convs
            .get(&(slug.to_string(), chat_id.to_string()))
            .cloned()
    }

    async fn agent_slug_for_conv(&self, conv_id: &str) -> Option<String> {
        let maps = self.conv_maps.read().await;
        maps.agent_space_convs
            .get(conv_id)
            .map(|meta| meta.agent_slug.clone())
    }

    async fn agent_id_for_conv(&self, conv_id: &str) -> Option<String> {
        let maps = self.conv_maps.read().await;
        maps.agent_space_convs
            .get(conv_id)
            .map(|meta| meta.agent_id.clone())
    }

    async fn agent_space_loop_id(&self, conv_id: &str) -> Option<String> {
        let maps = self.conv_maps.read().await;
        maps.agent_space_convs
            .get(conv_id)
            .map(|meta| meta.loop_id.clone())
    }

    async fn channel_for_conversation(&self, conv_id: &str) -> Option<String> {
        self.conv_maps
            .read()
            .await
            .channel_by_conv
            .get(conv_id)
            .cloned()
    }

    async fn agent_space_conv_for_slug(&self, slug: &str) -> Option<String> {
        self.conv_maps
            .read()
            .await
            .agent_space_by_slug
            .get(slug)
            .cloned()
    }

    async fn wait_disconnect(&self) -> crate::reconnect::Disconnect {
        self.disconnect_notify.notified().await;
        let ended = self.ended.lock().unwrap_or_else(|e| e.into_inner()).take();
        if ended == Some(crate::reconnect::Disconnect::Drain) {
            crate::reconnect::Disconnect::Drain
        } else if self.redirect_to.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
            crate::reconnect::Disconnect::Redirect
        } else {
            ended.unwrap_or(crate::reconnect::Disconnect::Dropped)
        }
    }

    async fn send_typing(
        &self,
        conversation_id: &str,
        is_typing: bool,
        status: Option<&str>,
    ) -> Result<(), CommError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(CommError::NotConnected);
        }
        let mut content = serde_json::json!({ "typing": is_typing });
        if let Some(s) = status {
            content["status"] = serde_json::Value::String(s.to_string());
        }
        self.send_on_conversation(conversation_id, "typing", content, true)
            .await
    }
}

// ── Background tasks ─────────────────────────────────────────────────

/// Join result update sent from read loop to the maps processor.
enum JoinUpdate {
    BotStream {
        key: String,
        conversation_id: String,
    },
    Channel(ChannelMeta, String),       // meta, conversation_id
    Dm(DmPeer, String),                 // peer, conversation_id
    AgentSpace(AgentSpaceMeta, String), // meta, conversation_id
    Embed {
        stream: String, // "embed:{oauthClientId}"
        conversation_id: String,
    },
}

/// Shared conversation maps updated by the join processor task.
#[derive(Default)]
struct ConvMaps {
    conv_by_key: HashMap<String, String>,
    channel_convs: HashMap<String, String>,
    channel_by_conv: HashMap<String, String>,
    channel_meta: HashMap<String, ChannelMeta>,
    dm_convs: HashMap<String, DmPeer>,
    dm_by_peer: HashMap<String, String>,
    agent_space_convs: HashMap<String, AgentSpaceMeta>, // conv_id → meta
    agent_space_by_slug: HashMap<String, String>,       // slug → conv_id
    agent_space_by_id: HashMap<String, String>,         // agent_id → conv_id
    embed_convs: HashMap<String, String>,               // conv_id → embed stream
    /// (agent_slug, chat_id) → conversation — per-chat agent spaces. The
    /// 'general' chat also lives in agent_space_by_slug/by_id for compat.
    agent_chat_convs: HashMap<(String, String), String>,
}

impl ConvMaps {
    fn apply(&mut self, update: JoinUpdate) {
        match update {
            JoinUpdate::BotStream {
                key,
                conversation_id,
            } => {
                self.conv_by_key.insert(key, conversation_id);
            }
            JoinUpdate::Channel(meta, conv_id) => {
                // Deduplicate: skip if this channel is already tracked with
                // the same conversation_id.
                if self.channel_convs.get(&meta.channel_id) == Some(&conv_id) {
                    return;
                }
                self.channel_by_conv
                    .insert(conv_id.clone(), meta.channel_id.clone());
                self.channel_convs.insert(meta.channel_id.clone(), conv_id);
                self.channel_meta.insert(meta.channel_id.clone(), meta);
            }
            JoinUpdate::Dm(peer, conv_id) => {
                self.dm_by_peer
                    .insert(peer.peer_id.clone(), conv_id.clone());
                self.dm_convs.insert(conv_id, peer);
            }
            JoinUpdate::AgentSpace(meta, conv_id) => {
                // The 'general' chat (or a pre-chats server sending no chat
                // id) is "the agent's conversation" — keep the legacy maps
                // pointing at it so existing lookups stay correct. Named
                // chats only register in the per-chat map.
                if meta.chat_id.is_empty() || meta.chat_id == "general" {
                    self.agent_space_by_slug
                        .insert(meta.agent_slug.clone(), conv_id.clone());
                    self.agent_space_by_id
                        .insert(meta.agent_id.clone(), conv_id.clone());
                }
                self.agent_chat_convs.insert(
                    (
                        meta.agent_slug.clone(),
                        if meta.chat_id.is_empty() {
                            "general".to_string()
                        } else {
                            meta.chat_id.clone()
                        },
                    ),
                    conv_id.clone(),
                );
                self.agent_space_convs.insert(conv_id, meta);
            }
            JoinUpdate::Embed {
                stream,
                conversation_id,
            } => {
                self.embed_convs.insert(conversation_id, stream);
            }
        }
    }
}

/// Encode an ACK frame for `acked_seq` on a conversation. One encoder, shared
/// by the public `ack()` and the read loop's automatic per-delivery ack.
fn encode_ack(conversation_id: &str, acked_seq: u64) -> Result<Vec<u8>, CommError> {
    let payload = serde_json::to_vec(&wire::AckPayload {
        conversation_id: conversation_id.to_string(),
        acked_seq,
    })
    .map_err(|e| CommError::Other(e.to_string()))?;

    frame::encode(
        Header {
            frame_type: frame::TYPE_ACK,
            ..Default::default()
        },
        &payload,
    )
    .map_err(|e| CommError::Other(e.to_string()))
}

/// Per-connection record of which conversations are the bot's own streams,
/// learned from their JOIN results, so each ack on one is also persisted as
/// that stream's offset. Owned by the read loop: what the hub replays for a
/// JOIN follows the JOIN result on the same socket, so the lookup never misses
/// it. (What the hub backfills before our JOIN — `installs` — it tracks by its
/// own record of our acks.)
struct StreamAcks {
    bot_id: String,
    offsets: Arc<dyn StreamOffsets>,
    stream_by_conv: HashMap<String, String>,
}

impl StreamAcks {
    fn joined(&mut self, conversation_id: &str, stream: &str) {
        self.stream_by_conv
            .insert(conversation_id.to_string(), stream.to_string());
    }

    fn acked(&self, conversation_id: &str, seq: u64) {
        if let Some(stream) = self.stream_by_conv.get(conversation_id) {
            self.offsets.record(&self.bot_id, stream, seq);
        }
    }
}

/// Read loop — receives WebSocket messages, decodes frames, dispatches.
/// The hub's answer to one dial and CONNECT.
enum Auth {
    /// AUTH_OK or a refusal other than a redirect: `data` is the frame.
    Answered {
        write: SplitSink<WsStream, WsMessage>,
        read: SplitStream<WsStream>,
        /// When CONNECT was sent: the lease a grant confers runs from here.
        connect_sent: std::time::Instant,
        data: Vec<u8>,
    },
    /// The account lives in another cell (`crate::cell`).
    Redirected(crate::cell::Redirect),
}

/// Dial `url`, send `connect_frame` and read the hub's answer: an HTTP 421,
/// an AUTH_FAIL `wrong_cell` carrying an address, or a close with
/// [`crate::cell::REDIRECT_CLOSE_CODE`] is a redirect.
async fn dial_and_auth(url: &str, connect_frame: &[u8]) -> Result<Auth, CommError> {
    // Each address the hostname resolves to is tried in a random order,
    // within its own connect and handshake limits (`tls::connect_ws`).
    let ws_stream = match tls::connect_ws(url).await {
        Ok((ws, _)) => ws,
        Err(e) => {
            return match crate::cell::Redirect::from_dial_error(&e) {
                Some(redirect) => Ok(Auth::Redirected(redirect)),
                None => Err(CommError::Other(format!("ws dial: {}", e))),
            };
        }
    };
    let (mut write, mut read) = ws_stream.split();

    let connect_sent = std::time::Instant::now();
    write
        .send(WsMessage::Binary(connect_frame.to_vec().into()))
        .await
        .map_err(|e| CommError::Other(format!("send connect: {}", e)))?;

    // Read AUTH response (with 10s timeout)
    let auth_msg = tokio::time::timeout(std::time::Duration::from_secs(10), read.next())
        .await
        .map_err(|_| CommError::Other("auth timeout".into()))?
        .ok_or_else(|| CommError::Other("connection closed during auth".into()))?
        .map_err(|e| CommError::Other(format!("read auth: {}", e)))?;

    let data = match auth_msg {
        WsMessage::Binary(data) => data.to_vec(),
        WsMessage::Close(Some(ref f)) => {
            if let Some(redirect) = crate::cell::Redirect::from_close(u16::from(f.code), f.reason.as_str()) {
                return Ok(Auth::Redirected(redirect));
            }
            if u16::from(f.code) == crate::reconnect::TRY_AGAIN_LATER_CLOSE_CODE {
                return Err(CommError::TryAgainLater);
            }
            return Err(CommError::Other(format!("unexpected ws message: {:?}", auth_msg)));
        }
        other => {
            return Err(CommError::Other(format!(
                "unexpected ws message: {:?}",
                other
            )));
        }
    };
    if let Ok((header, payload)) = frame::decode(&data) {
        if header.frame_type == frame::TYPE_AUTH_FAIL {
            let result: wire::AuthResultPayload = serde_json::from_slice(payload).unwrap_or_default();
            if result.reason == crate::cell::WRONG_CELL_REASON && !result.url.is_empty() {
                return Ok(Auth::Redirected(crate::cell::Redirect {
                    cell: result.cell,
                    url: result.url,
                }));
            }
        }
    }
    Ok(Auth::Answered { write, read, connect_sent, data })
}

async fn read_loop(
    mut read: SplitStream<WsStream>,
    handler: Option<MessageHandler>,
    join_tx: mpsc::Sender<JoinUpdate>,
    conv_maps: Arc<RwLock<ConvMaps>>,
    dedup: DedupWindow,
    cancel: tokio_util::sync::CancellationToken,
    devlog: Option<DevLog>,
    connected: Arc<AtomicBool>,
    disconnect_notify: Arc<Notify>,
    ended: Arc<std::sync::Mutex<Option<crate::reconnect::Disconnect>>>,
    redirect_to: Arc<std::sync::Mutex<Option<crate::cell::Redirect>>>,
    send_tx: mpsc::Sender<Vec<u8>>,
    mut stream_acks: StreamAcks,
) {
    if let Some(ref dl) = devlog {
        dl.event(&format!(
            "READ_LOOP started handler={}",
            if handler.is_some() { "SET" } else { "NONE" }
        ));
    }

    // An established connection that ends with no close frame (a read
    // error, an EOF, a silent hub) is a hard cut (crate::reconnect).
    let cut = || {
        *ended.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(crate::reconnect::Disconnect::from_close(None));
    };
    loop {
        tokio::select! {
            result = tokio::time::timeout(std::time::Duration::from_secs(120), read.next()) => {
                let msg = match result {
                    Ok(Some(Ok(m))) => m,
                    Ok(Some(Err(e))) => {
                        warn!(error = %e, "neboai read error");
                        if let Some(ref dl) = devlog {
                            dl.error(&format!("read error: {}", e));
                        }
                        cut();
                        break;
                    }
                    Ok(None) => {
                        if let Some(ref dl) = devlog {
                            dl.event("DISCONNECTED (stream ended)");
                        }
                        cut();
                        break;
                    }
                    Err(_) => {
                        // No data received in 120s (8x the 15s ping interval).
                        // Connection is likely dead (e.g. after system sleep/wake).
                        warn!("neboai read timeout (120s), treating as disconnect");
                        if let Some(ref dl) = devlog {
                            dl.event("READ_TIMEOUT (120s) — treating as disconnect");
                        }
                        cut();
                        break;
                    }
                };

                let data = match msg {
                    WsMessage::Binary(d) => d.to_vec(),
                    WsMessage::Ping(_) => {
                        if let Some(ref dl) = devlog {
                            dl.event("← PING");
                        }
                        continue;
                    }
                    WsMessage::Pong(_) => {
                        if let Some(ref dl) = devlog {
                            dl.event("← PONG");
                        }
                        continue;
                    }
                    WsMessage::Close(reason) => {
                        if let Some(ref dl) = devlog {
                            dl.event(&format!("CLOSE frame received: {:?}", reason));
                        }
                        let code = reason.as_ref().map(|f| u16::from(f.code));
                        // A close frame without a code still closed cleanly.
                        let how = crate::reconnect::Disconnect::from_close(Some(code.unwrap_or(1005)));
                        *ended.lock().unwrap_or_else(|e| e.into_inner()) = Some(how);
                        // A drain is a deploy moving the bot to another pod:
                        // planned, so it is told apart and redialed quietly.
                        if how == crate::reconnect::Disconnect::Drain {
                            info!("neboai: hub is draining this connection; redialing");
                        } else if let Some(redirect) = reason
                            .as_ref()
                            .and_then(|f| crate::cell::Redirect::from_close(u16::from(f.code), f.reason.as_str()))
                        {
                            // The account lives in another cell: the next
                            // connect dials there (crate::cell), quietly.
                            info!(cell = %redirect.cell, "neboai: hub names another cell; redialing there");
                            *redirect_to.lock().unwrap_or_else(|e| e.into_inner()) = Some(redirect);
                        } else {
                            info!(code = ?code, "neboai: hub closed the connection");
                        }
                        break;
                    }
                    _ => continue,
                };

                let (header, mut payload) = match frame::decode(&data) {
                    Ok(r) => r,
                    Err(e) => {
                        debug!(error = %e, "bad frame");
                        if let Some(ref dl) = devlog {
                            dl.error(&format!("bad frame: {}", e));
                        }
                        continue;
                    }
                };

                if let Some(ref dl) = devlog {
                    dl.event(&format!("← FRAME type={} payload={}b", header.frame_type, payload.len()));
                }

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
                            if let Some(ref dl) = devlog {
                                dl.event("── DEDUP skip (duplicate msg_id)");
                            }
                            continue;
                        }

                        let delivery: wire::DeliveryPayload = match serde_json::from_slice(payload) {
                            Ok(d) => d,
                            Err(e) => {
                                if let Some(ref dl) = devlog {
                                    dl.error(&format!("delivery parse failed: {}", e));
                                }
                                continue;
                            }
                        };

                        let mut metadata = HashMap::new();
                        // Lift the sender's nested content.metadata (string
                        // fields) — control frames like the web's Stop button
                        // (kind=stop) arrive here. Envelope fields win below.
                        if let Some(content_meta) = delivery
                            .content
                            .get("metadata")
                            .and_then(|m| m.as_object())
                        {
                            for (k, v) in content_meta {
                                if let Some(val) = v.as_str() {
                                    metadata.insert(k.clone(), val.to_string());
                                }
                            }
                        }
                        // The sender's own id for this message, the same on
                        // every send of it (an outbox resend included): what
                        // tells a second copy from a new message.
                        if let Some(client_id) = delivery
                            .content
                            .get("clientId")
                            .and_then(|v| v.as_str())
                            .filter(|v| !v.is_empty())
                        {
                            metadata.insert("clientId".to_string(), client_id.to_string());
                        }
                        if !delivery.agent_id.is_empty() {
                            metadata.insert("agent_id".to_string(), delivery.agent_id.clone());
                        }
                        if !delivery.agent_slug.is_empty() {
                            metadata.insert("agent_slug".to_string(), delivery.agent_slug.clone());
                        }
                        if !delivery.source_channel_id.is_empty() {
                            metadata.insert("source_channel_id".to_string(), delivery.source_channel_id.clone());
                        }
                        // Sender agent identity (envelope wins over any lifted
                        // content copy): which of the SENDING bot's employees
                        // spoke. Drives room attribution and agent-aware
                        // self-echo suppression.
                        if !delivery.from_agent_id.is_empty() {
                            metadata.insert("fromAgentId".to_string(), delivery.from_agent_id.clone());
                        }
                        if !delivery.from_agent_name.is_empty() {
                            metadata.insert("fromAgentName".to_string(), delivery.from_agent_name.clone());
                        }

                        let conv_id_str = uuid_from_bytes(&header.conversation_id);

                        // Deliveries on an embed conversation arrive on stream
                        // "chat" — tag the topic as "embed" (recorded at JOIN
                        // time) so the server routes them to the embed branch.
                        let topic = if conv_maps
                            .read()
                            .await
                            .embed_convs
                            .contains_key(&conv_id_str)
                        {
                            "embed".to_string()
                        } else {
                            delivery.stream.clone()
                        };

                        if let Some(ref dl) = devlog {
                            dl.inbound(
                                &delivery.stream,
                                &delivery.sender_id,
                                &delivery.agent_slug,
                                &conv_id_str,
                                &delivery.content.to_string(),
                            );
                        }

                        let msg = CommMessage {
                            id: uuid_from_bytes(&header.msg_id),
                            from: delivery.sender_id.clone(),
                            to: String::new(),
                            topic,
                            conversation_id: conv_id_str.clone(),
                            msg_type: CommMessageType::Message,
                            content: delivery.content.to_string(),
                            metadata,
                            timestamp: 0,
                            human_injected: false,
                            human_id: None,
                            task_id: None,
                            correlation_id: None,
                            task_status: None,
                            artifacts: vec![],
                            error: None,
                            attachments: delivery.content
                                .get("attachments")
                                .and_then(|v| serde_json::from_value(v.clone()).ok())
                                .unwrap_or_default(),
                        };

                        if let Some(ref h) = handler {
                            h(msg);

                            // Ack what we took. The server replays an agent
                            // space or auto-subscribed stream from this offset
                            // on the next connect, and we JOIN our own streams
                            // from the offset persisted here, so a conversation
                            // we never ack is one whose backlog can never come
                            // back — that is how inbound webhooks went missing.
                            // Ephemeral frames (presence, identity) carry no
                            // durable seq: never ack those.
                            if header.seq > 0 && !header.is_ephemeral() {
                                stream_acks.acked(&conv_id_str, header.seq);
                                match encode_ack(&conv_id_str, header.seq) {
                                    Ok(frame) => {
                                        if send_tx.try_send(frame).is_err() {
                                            debug!(
                                                conv_id = %conv_id_str,
                                                seq = header.seq,
                                                "ack not queued (send channel full or closed)"
                                            );
                                        }
                                    }
                                    Err(e) => debug!(error = %e, "ack encode failed"),
                                }
                            }
                        } else {
                            warn!(stream = %delivery.stream, "message dropped: no handler set");
                            if let Some(ref dl) = devlog {
                                dl.error(&format!("DROPPED (no handler): stream={}", delivery.stream));
                            }
                        }
                    }

                    frame::TYPE_JOIN_CONVERSATION => {
                        let result: wire::JoinResultPayload = match serde_json::from_slice(payload) {
                            Ok(r) => r,
                            Err(_) => continue,
                        };

                        if let Some(ref dl) = devlog {
                            let info = if result.conv_type == "embed" {
                                format!(
                                    "embed stream={} conv={}",
                                    result.stream,
                                    &result.conversation_id.get(..8).unwrap_or(&result.conversation_id),
                                )
                            } else if !result.agent_id.is_empty() {
                                format!(
                                    "agent_space agent={} conv={}",
                                    result.agent_slug,
                                    &result.conversation_id.get(..8).unwrap_or(&result.conversation_id),
                                )
                            } else if !result.channel_id.is_empty() {
                                format!(
                                    "channel={} conv={} loop={}",
                                    result.channel_name,
                                    &result.conversation_id.get(..8).unwrap_or(&result.conversation_id),
                                    &result.loop_id.get(..8).unwrap_or(&result.loop_id),
                                )
                            } else if !result.peer_id.is_empty() {
                                format!(
                                    "dm peer={} conv={}",
                                    result.peer_id,
                                    &result.conversation_id.get(..8).unwrap_or(&result.conversation_id),
                                )
                            } else {
                                format!(
                                    "stream conv={}",
                                    &result.conversation_id.get(..8).unwrap_or(&result.conversation_id),
                                )
                            };
                            dl.join_result(&info);
                        }

                        if result.conv_type == "embed" {
                            // Embed conversation join (pushed by the gateway when
                            // a publisher widget opens a conversation with this
                            // bot) — track it so deliveries get topic "embed".
                            let _ = join_tx
                                .send(JoinUpdate::Embed {
                                    stream: result.stream,
                                    conversation_id: result.conversation_id,
                                })
                                .await;
                        } else if !result.agent_id.is_empty() {
                            // Agent space join
                            let _ = join_tx
                                .send(JoinUpdate::AgentSpace(
                                    AgentSpaceMeta {
                                        agent_id: result.agent_id,
                                        agent_slug: result.agent_slug,
                                        loop_id: result.loop_id.clone(),
                                        chat_id: result.chat_id.clone(),
                                        chat_title: result.chat_title.clone(),
                                    },
                                    result.conversation_id,
                                ))
                                .await;
                        } else if !result.peer_id.is_empty() {
                            // DM join (still active for gateways that haven't migrated)
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
                        } else if !result.stream.is_empty() {
                            // Bot stream join — the result names its stream.
                            stream_acks.joined(&result.conversation_id, &result.stream);
                            let _ = join_tx
                                .send(JoinUpdate::BotStream {
                                    key: format!("{}:{}", result.bot_id, result.stream),
                                    conversation_id: result.conversation_id,
                                })
                                .await;
                        }
                    }

                    frame::TYPE_LEASE => {
                        let answer: wire::LeaseAnswer = match serde_json::from_slice(payload) {
                            Ok(a) => a,
                            Err(e) => {
                                debug!(error = %e, "unreadable lease answer");
                                continue;
                            }
                        };
                        let lease = crate::lease::process();
                        if answer.kind == "lease_ok" {
                            lease.renewed(answer.epoch, lease.instant_of(answer.t));
                            continue;
                        }
                        // Another process holds the bot now. The hub closes
                        // this connection; stop reading so the reconnect
                        // path asks again (and is refused while it lasts).
                        warn!(epoch = answer.epoch, "neboai: lease lost — another running copy of this bot holds it");
                        if let Some(ref dl) = devlog {
                            dl.event(&format!("LEASE_LOST epoch={}", answer.epoch));
                        }
                        lease.lost();
                        break;
                    }

                    frame::TYPE_REPLAY => {
                        if let Some(ref dl) = devlog {
                            dl.event(&format!("← REPLAY payload={}b", payload.len()));
                        }
                        debug!("replay frame received");
                    }

                    _ => {
                        if let Some(ref dl) = devlog {
                            dl.event(&format!("← UNKNOWN type={} payload={}b", header.frame_type, payload.len()));
                        }
                        debug!(frame_type = header.frame_type, "unhandled frame type");
                    }
                }
            }
            _ = cancel.cancelled() => {
                if let Some(ref dl) = devlog {
                    dl.event("CANCELLED (token cancelled)");
                }
                break;
            }
        }
    }
    // Signal disconnection so is_connected() returns false and reconnect kicks in.
    // If the exit was due to cancellation (intentional disconnect), don't notify —
    // the caller already knows. Only notify on unexpected drops (read error, stream end).
    let was_cancelled = cancel.is_cancelled();
    // Only update connected flag for unexpected disconnects (read error, stream end,
    // timeout). For intentional disconnects (cancel token), disconnect() already set
    // connected=false. Setting it here would race with connect() and clobber the NEW
    // connection's connected=true if the old read loop exits after reconnect completes.
    if !was_cancelled {
        connected.store(false, Ordering::SeqCst);
    }
    cancel.cancel();
    if !was_cancelled {
        disconnect_notify.notify_one();
    }
    info!("neboai read loop exited");
}

/// Write loop — sends queued frames and periodic pings. Each ping carries
/// the lease renewal while this process holds the bot's lease.
async fn write_loop(
    mut write: SplitSink<WsStream, WsMessage>,
    mut send_rx: mpsc::Receiver<Vec<u8>>,
    cancel: tokio_util::sync::CancellationToken,
    devlog: Option<DevLog>,
) {
    let lease = crate::lease::process();
    let mut ping_interval = tokio::time::interval(crate::lease::RENEW_EVERY);
    ping_interval.tick().await; // skip first immediate tick
    let mut last_ping_wall = std::time::SystemTime::now();

    loop {
        tokio::select! {
            msg = send_rx.recv() => {
                match msg {
                    Some(data) => {
                        if let Some(ref dl) = devlog {
                            dl.event(&format!("→ SEND frame={}b", data.len()));
                        }
                        if let Err(e) = write.send(WsMessage::Binary(data.into())).await {
                            warn!(error = %e, "neboai write error");
                            break;
                        }
                    }
                    None => {
                        // Send channel closed (disconnect or replaced by new connect).
                        // Exit cleanly so we don't keep pinging a ghost connection.
                        debug!("neboai send channel closed, exiting write loop");
                        break;
                    }
                }
            }
            _ = ping_interval.tick() => {
                // Detect wall-clock drift — if elapsed > 60s (4x the ping
                // interval), the system was likely asleep and the TCP
                // connection is dead.
                let now_wall = std::time::SystemTime::now();
                let elapsed = now_wall.duration_since(last_ping_wall).unwrap_or_default();
                last_ping_wall = now_wall;

                if elapsed > std::time::Duration::from_secs(60) {
                    warn!(
                        elapsed_secs = elapsed.as_secs(),
                        "neboai write loop detected sleep drift, exiting"
                    );
                    if let Some(ref dl) = devlog {
                        dl.event(&format!("SLEEP_DRIFT detected ({}s), exiting write loop", elapsed.as_secs()));
                    }
                    break;
                }

                if let Some(ref dl) = devlog {
                    dl.event("→ PING");
                }
                let renewal = lease
                    .held_epoch()
                    .and_then(|epoch| {
                        serde_json::to_vec(&wire::LeaseRenewal {
                            lease_epoch: epoch,
                            t: lease.stamp(std::time::Instant::now()),
                        })
                        .ok()
                    })
                    .unwrap_or_default();
                if let Err(e) = write.send(WsMessage::Ping(renewal.into())).await {
                    debug!(error = %e, "neboai ping error");
                    break;
                }
            }
            _ = cancel.cancelled() => break,
        }
    }
    // A drained process hands its lease back before it goes, so the next
    // process need not wait out the TTL.
    if let Some(frame) = lease_release_frame(lease) {
        let sent = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            write.send(WsMessage::Binary(frame.into())),
        )
        .await;
        match sent {
            Ok(Ok(())) => info!(epoch = lease.epoch(), "bot lease handed back to the hub"),
            Ok(Err(e)) => warn!(error = %e, "bot lease release not sent; it expires on its TTL"),
            Err(_) => warn!("bot lease release timed out (2s); it expires on its TTL"),
        }
    }
    // Send WebSocket Close frame so the gateway drops this connection immediately
    // (rather than waiting for its keepalive timeout to expire).
    let close_result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        write.send(WsMessage::Close(None)),
    )
    .await;
    if let Some(ref dl) = devlog {
        match close_result {
            Ok(Ok(())) => dl.event("CLOSE frame sent"),
            Ok(Err(e)) => dl.event(&format!("CLOSE frame failed: {e}")),
            Err(_) => dl.event("CLOSE frame timed out (2s)"),
        }
    }

    if let Some(ref dl) = devlog {
        dl.event("WRITE_LOOP exited");
    }
    debug!("neboai write loop exited");
}

// ── Helpers ──────────────────────────────────────────────────────────

/// The CLOSE frame that hands this process's lease back, once the drain has
/// released it (`Lease::release`) and while it is held.
fn lease_release_frame(lease: &crate::lease::Lease) -> Option<Vec<u8>> {
    lease.release_epoch()?;
    let payload = serde_json::to_vec(&wire::LeaseRelease { release_lease: true }).ok()?;
    frame::encode(
        Header {
            frame_type: frame::TYPE_CLOSE,
            ..Default::default()
        },
        &payload,
    )
    .ok()
}

/// Mirror of NeboLoop's `sanitizeChannelName` so find-by-name matches what the
/// server stores: lowercase, trim, spaces→'-', drop '.', keep [a-z0-9-],
/// collapse repeated '-', trim '-', cap at 80.
fn sanitize_channel_name(name: &str) -> String {
    let mut s: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c == ' ' { '-' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    s.trim_matches('-').chars().take(80).collect()
}

/// Format 16 bytes as a UUID string.
fn uuid_from_bytes(b: &[u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0],
        b[1],
        b[2],
        b[3],
        b[4],
        b[5],
        b[6],
        b[7],
        b[8],
        b[9],
        b[10],
        b[11],
        b[12],
        b[13],
        b[14],
        b[15],
    )
}

/// Derive REST API URL from a WebSocket gateway URL.
/// e.g. "wss://comms.neboai.com/ws" → "https://api.neboai.com"
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The drain's CLOSE frame: sent only once the lease is released while
    /// held, and it says `releaseLease` in the hub's frame format.
    #[test]
    fn a_released_lease_is_handed_back_on_a_close_frame() {
        let lease = crate::lease::Lease::new();
        lease.granted(3, std::time::Duration::from_secs(60), std::time::Instant::now());
        assert!(lease_release_frame(&lease).is_none(), "a running process keeps its lease");
        lease.release();
        let bytes = lease_release_frame(&lease).expect("a release frame");
        let (h, payload) = frame::decode(&bytes).unwrap();
        assert_eq!(h.frame_type, frame::TYPE_CLOSE);
        assert_eq!(payload, br#"{"releaseLease":true}"#);
    }

    #[derive(Default)]
    struct MemOffsets(Mutex<HashMap<String, u64>>);

    impl StreamOffsets for MemOffsets {
        fn acked(&self, bot_id: &str, stream: &str) -> u64 {
            let map = self.0.lock().unwrap();
            map.get(&format!("{bot_id}:{stream}")).copied().unwrap_or(0)
        }
        fn record(&self, bot_id: &str, stream: &str, seq: u64) {
            let mut map = self.0.lock().unwrap();
            let slot = map.entry(format!("{bot_id}:{stream}")).or_insert(0);
            *slot = (*slot).max(seq);
        }
    }

    fn server_frame(
        frame_type: u8,
        conversation_id: [u8; 16],
        seq: u64,
        payload: serde_json::Value,
    ) -> WsMessage {
        let bytes = frame::encode(
            Header {
                frame_type,
                conversation_id,
                seq,
                msg_id: [seq as u8; 16],
                ..Default::default()
            },
            &serde_json::to_vec(&payload).unwrap(),
        )
        .unwrap();
        WsMessage::Binary(bytes.into())
    }

    fn client_frame(msg: WsMessage) -> (Header, serde_json::Value) {
        let WsMessage::Binary(data) = msg else {
            panic!("expected a binary frame, got {msg:?}");
        };
        let (header, payload) = frame::decode(&data).unwrap();
        (header, serde_json::from_slice(payload).unwrap())
    }

    /// Every connect JOINs each of the bot's own streams — `channels/inbound`
    /// included — with its persisted acked offset, and a delivery handled on
    /// one of them is acked to the hub AND persisted as that stream's offset
    /// for the next connect.
    #[tokio::test]
    async fn joins_every_stream_from_its_offset_and_persists_acks() {
        let bot = "bot-under-test";
        let offsets = Arc::new(MemOffsets::default());
        offsets.record(bot, "chat", 5);
        offsets.record(bot, "channels/inbound", 2);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let chat_conv = [0x11u8; 16];

        let hub = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();

            let (connect, _) = client_frame(ws.next().await.unwrap().unwrap());
            assert_eq!(connect.frame_type, frame::TYPE_CONNECT);
            ws.send(server_frame(
                frame::TYPE_AUTH_OK,
                [0; 16],
                0,
                serde_json::json!({"ok": true}),
            ))
            .await
            .unwrap();

            let mut joins = HashMap::new();
            while joins.len() < BOT_STREAMS.len() {
                let (h, join) = client_frame(ws.next().await.unwrap().unwrap());
                assert_eq!(h.frame_type, frame::TYPE_JOIN_CONVERSATION);
                joins.insert(
                    join["stream"].as_str().unwrap().to_string(),
                    join["lastAckedSeq"].as_u64().unwrap_or(0),
                );
            }

            // The chat JOIN result, then a delivery on it (what a replay sends).
            ws.send(server_frame(
                frame::TYPE_JOIN_CONVERSATION,
                chat_conv,
                0,
                serde_json::json!({
                    "conversationId": uuid_from_bytes(&chat_conv),
                    "botId": bot,
                    "stream": "chat",
                }),
            ))
            .await
            .unwrap();
            ws.send(server_frame(
                frame::TYPE_MESSAGE_DELIVERY,
                chat_conv,
                9,
                serde_json::json!({"senderId": "owner", "stream": "chat", "content": {"text": "hi"}}),
            ))
            .await
            .unwrap();

            loop {
                let (h, ack) = client_frame(ws.next().await.unwrap().unwrap());
                if h.frame_type == frame::TYPE_ACK {
                    return (joins, ack);
                }
            }
        });

        let plugin = NeboAIPlugin::new(offsets.clone());
        plugin.set_message_handler(Arc::new(|_msg| {}));
        let config = HashMap::from([
            ("gateway".to_string(), format!("ws://{addr}/ws")),
            ("bot_id".to_string(), bot.to_string()),
            ("token".to_string(), "test-token".to_string()),
            ("api_server".to_string(), "http://127.0.0.1:9".to_string()),
        ]);
        plugin.connect(config).await.unwrap();

        let (joins, ack) = tokio::time::timeout(std::time::Duration::from_secs(10), hub)
            .await
            .expect("hub saw the joins and the ack")
            .unwrap();

        let expected: HashMap<String, u64> = BOT_STREAMS
            .iter()
            .map(|s| {
                let seq = match *s {
                    "chat" => 5,
                    "channels/inbound" => 2,
                    _ => 0,
                };
                (s.to_string(), seq)
            })
            .collect();
        assert_eq!(joins, expected);
        assert_eq!(ack["conversationId"], uuid_from_bytes(&chat_conv));
        assert_eq!(ack["ackedSeq"], 9);
        assert_eq!(
            offsets.acked(bot, "chat"),
            9,
            "the ack is the next connect's offset"
        );
        // The join processor applies JOIN results on its own task.
        let mut mapped = None;
        for _ in 0..50 {
            mapped = plugin.conversation_for_key(&format!("{bot}:chat")).await;
            if mapped.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(
            mapped,
            Some(uuid_from_bytes(&chat_conv)),
            "the JOIN result maps the stream it names"
        );
        plugin.disconnect().await.unwrap();
    }

    /// The comms close code is read in every build: a hub drain (1012) is
    /// told apart from any other close, and from a connection that just ends.
    #[tokio::test]
    async fn a_drain_close_is_told_apart_from_other_drops() {
        use tokio_tungstenite::tungstenite::protocol::CloseFrame;
        use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

        // A hub that grants the connection, then ends it with `close`
        // (None: drops the socket without a close frame).
        async fn ended_with(close: Option<CloseCode>) -> crate::reconnect::Disconnect {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                let (tcp, _) = listener.accept().await.unwrap();
                let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                let _ = ws.next().await; // CONNECT
                ws.send(server_frame(frame::TYPE_AUTH_OK, [0; 16], 0, serde_json::json!({"ok": true})))
                    .await
                    .unwrap();
                match close {
                    Some(code) => {
                        let frame = CloseFrame { code, reason: "drain".into() };
                        let _ = ws.send(WsMessage::Close(Some(frame))).await;
                        // Drain until the client answers the close.
                        while let Some(Ok(_)) = ws.next().await {}
                    }
                    None => drop(ws),
                }
            });
            let plugin = NeboAIPlugin::new(Arc::new(MemOffsets::default()));
            let config = HashMap::from([
                ("gateway".to_string(), format!("ws://{addr}/ws")),
                ("bot_id".to_string(), "bot-under-test".to_string()),
                ("token".to_string(), "test-token".to_string()),
                ("api_server".to_string(), "http://127.0.0.1:9".to_string()),
            ]);
            plugin.connect(config).await.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(10), plugin.wait_disconnect())
                .await
                .expect("the drop is reported")
        }

        use crate::reconnect::Disconnect;
        assert_eq!(ended_with(Some(CloseCode::Restart)).await, Disconnect::Drain);
        assert_eq!(ended_with(Some(CloseCode::Normal)).await, Disconnect::Dropped);
        assert_eq!(ended_with(Some(CloseCode::Error)).await, Disconnect::Dropped);
        assert_eq!(ended_with(None).await, Disconnect::Cut);
    }

    /// The hub's refusal of a removed bot is `Revoked`; any other refusal
    /// stays an ordinary failure the caller retries.
    #[tokio::test]
    async fn a_revoked_bot_is_told_apart_from_other_refusals() {
        async fn connect_refused_with(reason: &str) -> CommError {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let reason = reason.to_string();
            tokio::spawn(async move {
                let (tcp, _) = listener.accept().await.unwrap();
                let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                let (connect, _) = client_frame(ws.next().await.unwrap().unwrap());
                assert_eq!(connect.frame_type, frame::TYPE_CONNECT);
                ws.send(server_frame(
                    frame::TYPE_AUTH_FAIL,
                    [0; 16],
                    0,
                    serde_json::json!({"ok": false, "reason": reason}),
                ))
                .await
                .unwrap();
            });
            let plugin = NeboAIPlugin::new(Arc::new(MemOffsets::default()));
            let config = HashMap::from([
                ("gateway".to_string(), format!("ws://{addr}/ws")),
                ("bot_id".to_string(), "bot-under-test".to_string()),
                ("token".to_string(), "test-token".to_string()),
                ("api_server".to_string(), "http://127.0.0.1:9".to_string()),
            ]);
            plugin.connect(config).await.unwrap_err()
        }

        assert!(matches!(
            connect_refused_with(crate::REVOKED_REASON).await,
            CommError::Revoked
        ));
        assert!(matches!(
            connect_refused_with("stale token").await,
            CommError::AuthFailed(reason) if reason == "stale token"
        ));
    }

    /// How a fake cell answers each dial.
    #[derive(Clone)]
    enum FakeCell {
        /// AUTH_OK, then hold the connection until the client goes.
        Home,
        /// AUTH_OK, then close with 4421 naming `url` (the account moved).
        MovesAway(String),
        /// AUTH_FAIL `wrong_cell` naming `url`.
        WrongCell(String),
        /// A 4421 close naming `url` in place of the AUTH answer.
        ClosesTo(String),
        /// An HTTP 421 refusal of the upgrade naming `url`.
        Refuses(String),
    }

    async fn bind_cell() -> (tokio::net::TcpListener, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/ws", listener.local_addr().unwrap());
        (listener, url)
    }

    /// Serve every dial to `listener` as `cell`; the count is how many dials
    /// it took.
    fn serve_cell(
        listener: tokio::net::TcpListener,
        cell: FakeCell,
    ) -> Arc<std::sync::atomic::AtomicUsize> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio_tungstenite::tungstenite::protocol::CloseFrame;
        use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
        let dials = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = dials.clone();
        tokio::spawn(async move {
            loop {
                let (mut tcp, _) = listener.accept().await.unwrap();
                counted.fetch_add(1, Ordering::SeqCst);
                let cell = cell.clone();
                tokio::spawn(async move {
                    let redirect = |url: &str| serde_json::json!({"cell": "2", "url": url}).to_string();
                    if let FakeCell::Refuses(url) = &cell {
                        let mut head = [0u8; 4096];
                        let _ = tcp.read(&mut head).await;
                        let body = serde_json::json!({"error": "wrong_cell", "cell": "2", "url": url}).to_string();
                        let reply = format!(
                            "HTTP/1.1 421 Misdirected Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = tcp.write_all(reply.as_bytes()).await;
                        return;
                    }
                    let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                    let _ = ws.next().await; // CONNECT
                    let close_to = |url: &str| {
                        WsMessage::Close(Some(CloseFrame {
                            code: CloseCode::from(crate::cell::REDIRECT_CLOSE_CODE),
                            reason: redirect(url).into(),
                        }))
                    };
                    match cell {
                        FakeCell::Home | FakeCell::MovesAway(_) => {
                            let ok = server_frame(frame::TYPE_AUTH_OK, [0; 16], 0, serde_json::json!({"ok": true}));
                            ws.send(ok).await.unwrap();
                            if let FakeCell::MovesAway(url) = &cell {
                                let _ = ws.send(close_to(url)).await;
                            }
                        }
                        FakeCell::WrongCell(url) => {
                            let fail = serde_json::json!({"ok": false, "reason": "wrong_cell", "cell": "2", "url": url});
                            let _ = ws.send(server_frame(frame::TYPE_AUTH_FAIL, [0; 16], 0, fail)).await;
                        }
                        FakeCell::ClosesTo(url) => {
                            let _ = ws.send(close_to(&url)).await;
                        }
                        FakeCell::Refuses(_) => unreachable!(),
                    }
                    while let Some(Ok(_)) = ws.next().await {}
                });
            }
        });
        dials
    }

    fn cell_config(gateway: &str) -> HashMap<String, String> {
        HashMap::from([
            ("gateway".to_string(), gateway.to_string()),
            ("bot_id".to_string(), "bot-under-test".to_string()),
            ("token".to_string(), "test-token".to_string()),
            ("api_server".to_string(), "http://127.0.0.1:9".to_string()),
        ])
    }

    /// A cell the account does not live in names the one it does, by any of
    /// its three signals; the bot connects there, quietly: nothing reaches
    /// the owner's handlers and no disconnect is reported.
    #[tokio::test]
    async fn a_cell_redirect_is_followed_quietly() {
        for wrong in [FakeCell::WrongCell as fn(String) -> FakeCell, FakeCell::ClosesTo, FakeCell::Refuses] {
            let (home_listener, home_url) = bind_cell().await;
            let (wrong_listener, wrong_url) = bind_cell().await;
            let home = serve_cell(home_listener, FakeCell::Home);
            let wrong = serve_cell(wrong_listener, wrong(home_url));

            let plugin = NeboAIPlugin::new(Arc::new(MemOffsets::default()));
            let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let counted = seen.clone();
            plugin.set_message_handler(Arc::new(move |_| {
                counted.fetch_add(1, Ordering::SeqCst);
            }));
            plugin.connect(cell_config(&wrong_url)).await.expect("connected in the home cell");
            assert!(plugin.is_connected());
            assert_eq!(wrong.load(Ordering::SeqCst), 1);
            assert_eq!(home.load(Ordering::SeqCst), 1);
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(300), plugin.wait_disconnect())
                    .await
                    .is_err(),
                "a followed redirect is not a disconnect"
            );
            assert_eq!(seen.load(Ordering::SeqCst), 0, "nothing reaches the owner");
            plugin.disconnect().await.ok();
        }
    }

    /// A close naming another cell mid-session is a quiet redirect, and the
    /// next connect dials that cell first.
    #[tokio::test]
    async fn an_account_moved_mid_session_redials_its_new_cell() {
        let (home_listener, home_url) = bind_cell().await;
        let (old_listener, old_url) = bind_cell().await;
        let home = serve_cell(home_listener, FakeCell::Home);
        let old = serve_cell(old_listener, FakeCell::MovesAway(home_url));

        let plugin = NeboAIPlugin::new(Arc::new(MemOffsets::default()));
        plugin.connect(cell_config(&old_url)).await.unwrap();
        let how = tokio::time::timeout(std::time::Duration::from_secs(10), plugin.wait_disconnect())
            .await
            .expect("the close is reported");
        assert_eq!(how, crate::reconnect::Disconnect::Redirect);
        plugin.connect(cell_config(&old_url)).await.unwrap();
        assert_eq!(old.load(Ordering::SeqCst), 1, "the old cell is not dialed again");
        assert_eq!(home.load(Ordering::SeqCst), 1);
        plugin.disconnect().await.ok();
    }

    /// Cells that keep pointing at each other are followed twice, then the
    /// connect fails like any refusal and the caller backs off.
    #[tokio::test]
    async fn a_redirect_loop_is_capped() {
        let (a_listener, a_url) = bind_cell().await;
        let (b_listener, b_url) = bind_cell().await;
        let (c_listener, c_url) = bind_cell().await;
        let a = serve_cell(a_listener, FakeCell::WrongCell(b_url));
        let b = serve_cell(b_listener, FakeCell::WrongCell(c_url));
        let c = serve_cell(c_listener, FakeCell::WrongCell(a_url.clone()));

        let plugin = NeboAIPlugin::new(Arc::new(MemOffsets::default()));
        let err = plugin.connect(cell_config(&a_url)).await.unwrap_err();
        assert!(matches!(err, CommError::Other(ref m) if m.contains("not following")), "{err:?}");
        let dials = [&a, &b, &c].map(|n| n.load(Ordering::SeqCst));
        assert_eq!(dials, [1, 1, 1], "two redirects followed, the third refused");
        assert!(!plugin.is_connected());
    }

    /// A redirect to a host outside NeboAI is never dialed.
    #[tokio::test]
    async fn a_redirect_to_a_foreign_host_is_refused() {
        for foreign in ["wss://evil.example/ws", "wss://neboai.com.evil.example/ws", "ws://10.0.0.1/ws"] {
            let (listener, url) = bind_cell().await;
            let dials = serve_cell(listener, FakeCell::WrongCell(foreign.to_string()));
            let plugin = NeboAIPlugin::new(Arc::new(MemOffsets::default()));
            let err = plugin.connect(cell_config(&url)).await.unwrap_err();
            assert!(matches!(err, CommError::Other(ref m) if m.contains("not following")), "{foreign}: {err:?}");
            assert_eq!(dials.load(Ordering::SeqCst), 1);
        }
    }

    /// A `wrong_cell` refusal that names no address is an ordinary refusal.
    #[tokio::test]
    async fn a_wrong_cell_without_an_address_is_an_ordinary_refusal() {
        let (listener, url) = bind_cell().await;
        serve_cell(listener, FakeCell::WrongCell(String::new()));
        let plugin = NeboAIPlugin::new(Arc::new(MemOffsets::default()));
        let err = plugin.connect(cell_config(&url)).await.unwrap_err();
        assert!(matches!(err, CommError::AuthFailed(ref m) if m == "wrong_cell"), "{err:?}");
    }

    /// How a fake gateway answers one dial.
    #[derive(Clone, Copy)]
    enum Busy {
        /// AUTH_FAIL with this reason.
        Refuses(&'static str),
        /// Close 1013 Try Again Later before answering.
        TryAgainLater,
        /// AUTH_OK.
        Grants,
    }

    /// A hub that answers its dials in the order of `script`, recording the
    /// token each CONNECT carried and when it arrived.
    async fn busy_gateway(
        script: Vec<Busy>,
    ) -> (String, Arc<Mutex<Vec<(String, std::time::Instant)>>>) {
        use tokio_tungstenite::tungstenite::protocol::CloseFrame;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/ws", listener.local_addr().unwrap());
        let connects = Arc::new(Mutex::new(Vec::new()));
        let seen = connects.clone();
        tokio::spawn(async move {
            for answer in script {
                let (tcp, _) = listener.accept().await.unwrap();
                let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                let (connect, payload) = client_frame(ws.next().await.unwrap().unwrap());
                assert_eq!(connect.frame_type, frame::TYPE_CONNECT);
                let token = payload["token"].as_str().unwrap_or("").to_string();
                seen.lock().unwrap().push((token, std::time::Instant::now()));
                let reply = match answer {
                    Busy::Refuses(reason) => server_frame(
                        frame::TYPE_AUTH_FAIL,
                        [0; 16],
                        0,
                        serde_json::json!({"ok": false, "reason": reason}),
                    ),
                    Busy::TryAgainLater => WsMessage::Close(Some(CloseFrame {
                        code: crate::reconnect::TRY_AGAIN_LATER_CLOSE_CODE.into(),
                        reason: "try again later".into(),
                    })),
                    Busy::Grants => server_frame(frame::TYPE_AUTH_OK, [0; 16], 0, serde_json::json!({"ok": true})),
                };
                let _ = ws.send(reply).await;
                if matches!(answer, Busy::Grants) {
                    tokio::spawn(async move { while let Some(Ok(_)) = ws.next().await {} });
                }
            }
        });
        (url, connects)
    }

    /// A hub too busy to answer (`lease_unavailable` during a lease-store
    /// failover, or a 1013 close) is dialed again with the SAME token, never
    /// a refreshed one, each time after a jittered, doubling wait, and the
    /// connection comes up. Only a refused token asks for a refresh.
    #[tokio::test]
    async fn a_busy_hub_is_dialed_again_after_a_jittered_wait_with_the_same_token() {
        for busy in [Busy::Refuses("lease_unavailable"), Busy::TryAgainLater] {
            let (url, connects) = busy_gateway(vec![busy, busy, Busy::Grants]).await;
            let plugin = NeboAIPlugin::new(Arc::new(MemOffsets::default()));
            let waits = Arc::new(Mutex::new(Vec::new()));
            let recorded = waits.clone();
            crate::reconnect::dial_through_busy(
                || plugin.connect(cell_config(&url)),
                |wait| {
                    recorded.lock().unwrap().push(wait);
                    tokio::time::sleep(wait)
                },
            )
            .await
            .expect("connected once the hub had room");
            assert!(plugin.is_connected());

            let connects = connects.lock().unwrap().clone();
            let waits = waits.lock().unwrap().clone();
            assert_eq!(connects.len(), 3, "two refusals, then the grant");
            assert!(connects.iter().all(|(token, _)| token == "test-token"), "the token is never refreshed");
            assert_eq!(waits.len(), 2);
            for (n, ceiling) in [1u64, 2].into_iter().enumerate() {
                assert!(waits[n] <= std::time::Duration::from_secs(ceiling), "wait {n}: {:?}", waits[n]);
                let spaced = connects[n + 1].1 - connects[n].1;
                assert!(spaced >= waits[n], "dial {} came {spaced:?} after the last, before its {:?} wait", n + 1, waits[n]);
            }
            plugin.disconnect().await.ok();
        }

        // Read by the reason code: busy, a refused token, anything else.
        use crate::ConnectRefusal;
        for reason in ["lease_unavailable", "lease_required", "auth_unavailable"] {
            assert_eq!(CommError::AuthFailed(reason.into()).connect_refusal(), ConnectRefusal::Busy, "{reason}");
        }
        assert_eq!(CommError::TryAgainLater.connect_refusal(), ConnectRefusal::Busy);
        for reason in crate::TOKEN_REFUSALS {
            assert_eq!(CommError::AuthFailed(reason.into()).connect_refusal(), ConnectRefusal::Token, "{reason}");
        }
        for other in [
            CommError::AuthFailed("bot ownership mismatch".into()),
            CommError::LeaseHeld,
            CommError::Revoked,
            CommError::Other("auth failed: lease_unavailable".into()),
        ] {
            assert_eq!(other.connect_refusal(), ConnectRefusal::Other, "{other:?}");
        }
    }

    /// A busy hub is dialed again only so many times; then the caller's own
    /// backoff takes over. A refusal that is not busy is never redialed here.
    #[tokio::test]
    async fn busy_redials_are_capped_and_other_refusals_pass_straight_through() {
        let (url, connects) = busy_gateway(vec![Busy::TryAgainLater; crate::reconnect::BUSY_REDIALS + 1]).await;
        let plugin = NeboAIPlugin::new(Arc::new(MemOffsets::default()));
        let err = crate::reconnect::dial_through_busy(|| plugin.connect(cell_config(&url)), |_| async {})
            .await
            .unwrap_err();
        assert!(matches!(err, CommError::TryAgainLater), "{err:?}");
        assert_eq!(connects.lock().unwrap().len(), crate::reconnect::BUSY_REDIALS + 1);

        let (url, connects) = busy_gateway(vec![Busy::Refuses("stale token")]).await;
        let err = crate::reconnect::dial_through_busy(|| plugin.connect(cell_config(&url)), |_| async {})
            .await
            .unwrap_err();
        assert!(matches!(err, CommError::AuthFailed(ref r) if r == "stale token"), "{err:?}");
        assert_eq!(err.connect_refusal(), crate::ConnectRefusal::Token);
        assert_eq!(connects.lock().unwrap().len(), 1, "a refused token is not redialed with the same token");
    }
}
