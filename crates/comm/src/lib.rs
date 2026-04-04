pub mod api;
pub mod api_types;
pub mod compress;
pub mod dedup;
pub mod devlog;
pub mod frame;
mod loopback;
mod manager;
pub mod neboloop;
mod types;
pub mod ulid;
pub mod wire;

pub use loopback::LoopbackPlugin;
pub use manager::PluginManager;
pub use neboloop::NeboLoopPlugin;
pub use types::*;

use std::collections::HashMap;

/// CommPlugin defines the interface for communication transport plugins.
/// Plugins run in-process. Implementations include loopback (testing),
/// neboloop WebSocket, etc.
#[async_trait::async_trait]
pub trait CommPlugin: Send + Sync {
    /// Plugin identity.
    fn name(&self) -> &str;
    fn version(&self) -> &str;

    /// Lifecycle.
    async fn connect(&self, config: HashMap<String, String>) -> Result<(), CommError>;
    async fn disconnect(&self) -> Result<(), CommError>;
    fn is_connected(&self) -> bool;

    /// Messaging.
    async fn send(&self, msg: CommMessage) -> Result<(), CommError>;
    async fn subscribe(&self, topic: &str) -> Result<(), CommError>;
    async fn unsubscribe(&self, topic: &str) -> Result<(), CommError>;

    /// Registration with the comm network.
    async fn register(&self, agent_id: &str, card: &AgentCard) -> Result<(), CommError>;
    async fn deregister(&self) -> Result<(), CommError>;

    /// Message handler (set by PluginManager).
    fn set_message_handler(&self, handler: MessageHandler);

    /// List loop channels this bot belongs to.
    async fn list_channels(&self) -> Result<Vec<LoopChannelInfo>, CommError> {
        Err(CommError::Other("not supported".into()))
    }

    /// List loops this bot belongs to.
    async fn list_loops(&self) -> Result<Vec<LoopInfo>, CommError> {
        Err(CommError::Other("not supported".into()))
    }

    /// Get info for a single loop by ID.
    async fn get_loop_info(&self, _loop_id: &str) -> Result<LoopInfo, CommError> {
        Err(CommError::Other("not supported".into()))
    }

    /// List messages in a channel.
    async fn list_channel_messages(
        &self,
        _channel_id: &str,
        _limit: usize,
    ) -> Result<Vec<ChannelMessageItem>, CommError> {
        Err(CommError::Other("not supported".into()))
    }

    /// List members of a channel.
    async fn list_channel_members(
        &self,
        _channel_id: &str,
    ) -> Result<Vec<ChannelMemberItem>, CommError> {
        Err(CommError::Other("not supported".into()))
    }

    /// Retrieve and consume a rotated auth token (if the gateway issued one).
    /// Returns `None` for plugins that don't support token rotation.
    async fn take_rotated_token(&self) -> Option<String> {
        None
    }

    /// Look up the agent slug for a conversation ID (agent_space detection).
    /// Returns the slug if this conversation belongs to a registered agent space.
    async fn agent_slug_for_conv(&self, _conv_id: &str) -> Option<String> {
        None
    }

    /// Look up the loop_id for an agent_space conversation.
    /// Returns the loop_id if this conversation belongs to a registered agent space.
    async fn agent_space_loop_id(&self, _conv_id: &str) -> Option<String> {
        None
    }

    /// Look up the NeboLoop conversation ID for an agent by slug.
    /// Used for forwarding local agent responses to NeboLoop.
    async fn agent_space_conv_for_slug(&self, _slug: &str) -> Option<String> {
        None
    }

    /// Wait for an unexpected disconnect (read loop failure).
    /// Default implementation never returns (plugin doesn't support disconnect notification).
    async fn wait_disconnect(&self) {
        std::future::pending::<()>().await;
    }
}

/// Trait for channel providers that can receive streamed agent responses.
/// NeboLoop implements this via CommPlugin.send(). Future channel plugins
/// (Slack, Discord) implement this to receive responses from chat_dispatch.
#[async_trait::async_trait]
pub trait ChannelProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn send_response(&self, msg: CommMessage) -> Result<(), CommError>;
}
