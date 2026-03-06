pub mod api;
pub mod api_types;
pub mod compress;
pub mod dedup;
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
}

/// Optional trait for plugins that can list loop channels.
#[async_trait::async_trait]
pub trait LoopChannelLister: CommPlugin {
    async fn list_loop_channels(&self) -> Result<Vec<LoopChannelInfo>, CommError>;
}

/// Optional trait for plugins that can list loops.
#[async_trait::async_trait]
pub trait LoopLister: CommPlugin {
    async fn list_loops(&self) -> Result<Vec<LoopInfo>, CommError>;
}

/// Optional trait for plugins that can get a single loop by ID.
#[async_trait::async_trait]
pub trait LoopGetter: CommPlugin {
    async fn get_loop_info(&self, loop_id: &str) -> Result<LoopInfo, CommError>;
}

/// Optional trait for plugins that can list channel messages.
#[async_trait::async_trait]
pub trait ChannelMessageLister: CommPlugin {
    async fn list_channel_messages(
        &self,
        channel_id: &str,
        limit: usize,
    ) -> Result<Vec<ChannelMessageItem>, CommError>;
}

/// Optional trait for plugins that can list channel members.
#[async_trait::async_trait]
pub trait ChannelMemberLister: CommPlugin {
    async fn list_channel_members(
        &self,
        channel_id: &str,
    ) -> Result<Vec<ChannelMemberItem>, CommError>;
}
