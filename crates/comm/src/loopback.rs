use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

use tracing::{debug, info, warn};

use crate::{AgentCard, CommError, CommMessage, CommPlugin, MessageHandler};

/// In-memory comm plugin for testing. Delivers sent messages back to the handler.
pub struct LoopbackPlugin {
    inner: RwLock<Inner>,
}

struct Inner {
    handler: Option<MessageHandler>,
    connected: bool,
    topics: HashSet<String>,
    agent_id: String,
}

impl LoopbackPlugin {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Inner {
                handler: None,
                connected: false,
                topics: HashSet::new(),
                agent_id: String::new(),
            }),
        }
    }

    /// Simulate receiving a message from the network (for testing).
    pub fn inject_message(&self, msg: CommMessage) {
        let inner = self.inner.read().unwrap();
        let handler = inner.handler.clone();
        let subscribed = inner.topics.contains(&msg.topic);
        drop(inner);

        match handler {
            None => warn!("loopback: no handler set, dropping message"),
            Some(h) if !subscribed => {
                warn!(topic = msg.topic.as_str(), "loopback: not subscribed to topic, dropping message");
            }
            Some(h) => h(msg),
        }
    }
}

impl Default for LoopbackPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl CommPlugin for LoopbackPlugin {
    fn name(&self) -> &str {
        "loopback"
    }
    fn version(&self) -> &str {
        "1.0.0"
    }

    async fn connect(&self, _config: HashMap<String, String>) -> Result<(), CommError> {
        let mut inner = self.inner.write().unwrap();
        inner.connected = true;
        info!("loopback connected");
        Ok(())
    }

    async fn disconnect(&self) -> Result<(), CommError> {
        let mut inner = self.inner.write().unwrap();
        inner.connected = false;
        inner.topics.clear();
        info!("loopback disconnected");
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.inner.read().unwrap().connected
    }

    async fn send(&self, msg: CommMessage) -> Result<(), CommError> {
        let inner = self.inner.read().unwrap();
        if !inner.connected {
            return Err(CommError::NotConnected);
        }
        debug!(from = msg.from.as_str(), to = msg.to.as_str(), topic = msg.topic.as_str(), "loopback message sent");
        Ok(())
    }

    async fn subscribe(&self, topic: &str) -> Result<(), CommError> {
        let mut inner = self.inner.write().unwrap();
        if !inner.connected {
            return Err(CommError::NotConnected);
        }
        inner.topics.insert(topic.to_string());
        debug!(topic, "loopback subscribed");
        Ok(())
    }

    async fn unsubscribe(&self, topic: &str) -> Result<(), CommError> {
        let mut inner = self.inner.write().unwrap();
        if !inner.connected {
            return Err(CommError::NotConnected);
        }
        inner.topics.remove(topic);
        debug!(topic, "loopback unsubscribed");
        Ok(())
    }

    async fn register(&self, agent_id: &str, card: &AgentCard) -> Result<(), CommError> {
        let mut inner = self.inner.write().unwrap();
        inner.agent_id = agent_id.to_string();
        info!(agent = agent_id, skills = card.skills.len(), "loopback registered agent");
        Ok(())
    }

    async fn deregister(&self) -> Result<(), CommError> {
        let mut inner = self.inner.write().unwrap();
        inner.agent_id.clear();
        Ok(())
    }

    fn set_message_handler(&self, handler: MessageHandler) {
        let mut inner = self.inner.write().unwrap();
        inner.handler = Some(handler);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn test_loopback_lifecycle() {
        let plugin = LoopbackPlugin::new();
        assert!(!plugin.is_connected());

        plugin.connect(HashMap::new()).await.unwrap();
        assert!(plugin.is_connected());

        plugin.subscribe("test-topic").await.unwrap();
        plugin.disconnect().await.unwrap();
        assert!(!plugin.is_connected());
    }

    #[tokio::test]
    async fn test_loopback_inject() {
        let plugin = LoopbackPlugin::new();
        let received = Arc::new(AtomicBool::new(false));
        let received2 = received.clone();

        plugin.connect(HashMap::new()).await.unwrap();
        plugin.subscribe("test").await.unwrap();
        plugin.set_message_handler(Arc::new(move |_msg| {
            received2.store(true, Ordering::SeqCst);
        }));

        plugin.inject_message(CommMessage {
            id: "1".into(),
            from: "a".into(),
            to: "b".into(),
            topic: "test".into(),
            conversation_id: String::new(),
            msg_type: crate::CommMessageType::Message,
            content: "hello".into(),
            metadata: HashMap::new(),
            timestamp: 0,
            human_injected: false,
            human_id: None,
            task_id: None,
            correlation_id: None,
            task_status: None,
            artifacts: vec![],
            error: None,
        });

        assert!(received.load(Ordering::SeqCst));
    }
}
