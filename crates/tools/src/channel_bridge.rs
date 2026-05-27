//! Channel-plugin bridge registry — one entry per `(agent_id, plugin_slug)` pair.
//!
//! Each entry is a handle to a long-running channel-plugin sidecar's stdin
//! (e.g. `slack bridge --listen`). When an agent invokes
//! `plugin slack post …` / `plugin slack upload …` / `plugin slack dm …`,
//! `plugin_tool::handle_exec` looks up the bridge here and forwards the op
//! as one NDJSON line on the bridge's stdin — instead of spawning a fresh
//! one-shot CLI invocation that would race the bridge for the same upstream
//! socket.
//!
//! There is ONE canonical pathway per channel for every messaging operation;
//! see `docs/publishers-guide/channel-plugins.md`. CLI subcommands on channel
//! plugins are reserved for stateless ops (`auth`, `init`, `doctor`).
//!
//! ## Wiring
//!
//! - `server::lib::start_server` constructs the registry, stores it in
//!   `AppState.channel_bridges`, and registers it via [`set_channel_bridges`]
//!   so producers/consumers can reach it without an AppState back-reference.
//! - `agent::agent_worker::channel_loop` (producer) inserts a handle when it
//!   spawns the bridge sidecar and removes it on child exit/cancel.
//! - `tools::plugin_tool::handle_exec` (consumer) reads the handle to route
//!   messaging ops through the bridge's stdin.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use tokio::sync::{Mutex, RwLock, mpsc, oneshot};

/// Outcome of one bridge op, surfaced asynchronously from the bridge's
/// stdout (`event: "op_result"` NDJSON line). The agent-side caller
/// (`plugin_tool::route_through_bridge`) awaits this so the tool result
/// reflects what actually happened on the platform, not just that the
/// op was queued on stdin.
#[derive(Debug, Clone)]
pub struct OpResult {
    pub ok: bool,
    pub error: Option<String>,
}

/// Map of in-flight ops keyed by `req_id`. The producer
/// (`plugin_tool::route_through_bridge`) inserts a sender before writing the
/// op JSON; the consumer (`agent_worker::channel_loop`'s stdout reader) takes
/// the sender out when it sees the matching `op_result` event and forwards
/// the outcome back to the producer.
pub type PendingOps = Arc<Mutex<HashMap<String, oneshot::Sender<OpResult>>>>;

/// Handle for one running channel-plugin bridge — the agent-side end of its
/// stdin pipe + a registry of in-flight ops awaiting their result. Sending a
/// JSON value forwards one NDJSON line into the bridge sidecar, where the
/// `op` dispatcher routes it (reply/post/upload/dm). For ops the agent
/// initiated explicitly via `plugin_tool`, a `req_id` is included on the wire
/// so the bridge's eventual `op_result` event can be correlated back here.
#[derive(Clone)]
pub struct ChannelBridgeHandle {
    pub stdin_tx: mpsc::Sender<serde_json::Value>,
    pub agent_id: String,
    pub plugin_slug: String,
    pub pending_ops: PendingOps,
}

/// Create an empty `pending_ops` map. Each `ChannelBridgeHandle` gets its own.
pub fn new_pending_ops() -> PendingOps {
    Arc::new(Mutex::new(HashMap::new()))
}

/// Registry keyed by `{agent_id}:{plugin_slug}`. Matches the per-agent
/// `channel_loop` spawn key — each agent can independently enable a channel
/// plugin with its own credentials.
pub type ChannelBridgeRegistry = Arc<RwLock<HashMap<String, ChannelBridgeHandle>>>;

/// Build the registry key for a given (agent, plugin) pair.
pub fn channel_bridge_key(agent_id: &str, plugin_slug: &str) -> String {
    format!("{agent_id}:{plugin_slug}")
}

/// Create an empty registry. Called once at startup; the same `Arc` is shared
/// by `AppState.channel_bridges` and the global singleton wired by
/// [`set_channel_bridges`].
pub fn new_channel_bridge_registry() -> ChannelBridgeRegistry {
    Arc::new(RwLock::new(HashMap::new()))
}

static CHANNEL_BRIDGES: OnceLock<ChannelBridgeRegistry> = OnceLock::new();

/// Wire the global channel-bridge registry. Called once after the
/// `ChannelBridgeRegistry` is constructed (alongside AppState).
pub fn set_channel_bridges(registry: ChannelBridgeRegistry) {
    let _ = CHANNEL_BRIDGES.set(registry);
}

/// Borrow the global channel-bridge registry, if wired.
///
/// Returns `None` only during early startup, before `set_channel_bridges`
/// has run. Producers and consumers should treat `None` as "no bridges yet"
/// — not an error, just nothing registered.
pub fn channel_bridges() -> Option<&'static ChannelBridgeRegistry> {
    CHANNEL_BRIDGES.get()
}
