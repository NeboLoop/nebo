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
}
