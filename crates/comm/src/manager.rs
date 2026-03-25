use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::{CommError, CommMessage, CommPlugin, ManagerStatus, MessageHandler};

/// Manages loaded comm plugins and routes messages.
/// Only one plugin is active at a time; all messages route through it.
pub struct PluginManager {
    inner: RwLock<Inner>,
}

struct Inner {
    plugins: HashMap<String, Arc<dyn CommPlugin>>,
    active: Option<Arc<dyn CommPlugin>>,
    handler: Option<MessageHandler>,
    topics: Vec<String>,
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Inner {
                plugins: HashMap::new(),
                active: None,
                handler: None,
                topics: Vec::new(),
            }),
        }
    }

    /// Add a plugin to the manager (does not activate it).
    pub async fn register(&self, plugin: Arc<dyn CommPlugin>) {
        let mut inner = self.inner.write().await;
        info!(plugin = plugin.name(), version = plugin.version(), "registered comm plugin");
        inner.plugins.insert(plugin.name().to_string(), plugin);
    }

    /// Remove a plugin. Disconnects it if active.
    pub async fn unregister(&self, name: &str) {
        let mut inner = self.inner.write().await;
        if let Some(ref active) = inner.active {
            if active.name() == name {
                let _ = active.disconnect().await;
                inner.active = None;
            }
        }
        inner.plugins.remove(name);
        info!(plugin = name, "unregistered comm plugin");
    }

    /// Activate a specific plugin by name.
    pub async fn set_active(&self, name: &str) -> Result<(), CommError> {
        let mut inner = self.inner.write().await;
        let plugin = inner
            .plugins
            .get(name)
            .cloned()
            .ok_or_else(|| CommError::PluginNotFound(name.to_string()))?;

        // Disconnect current if different
        if let Some(ref active) = inner.active {
            if active.name() != name {
                if let Err(e) = active.disconnect().await {
                    warn!(plugin = active.name(), error = %e, "failed to disconnect plugin");
                }
            }
        }

        // Wire the handler
        if let Some(ref handler) = inner.handler {
            plugin.set_message_handler(handler.clone());
        }

        inner.active = Some(plugin);
        info!(plugin = name, "active comm plugin set");
        Ok(())
    }

    /// Get the name of the active plugin.
    pub async fn active_name(&self) -> Option<String> {
        let inner = self.inner.read().await;
        inner.active.as_ref().map(|p| p.name().to_string())
    }

    /// Send a message through the active plugin.
    pub async fn send(&self, msg: CommMessage) -> Result<(), CommError> {
        let inner = self.inner.read().await;
        let active = inner.active.as_ref().ok_or(CommError::NoActivePlugin)?;
        if !active.is_connected() {
            return Err(CommError::NotConnected);
        }
        active.send(msg).await
    }

    /// Subscribe to a topic on the active plugin.
    pub async fn subscribe(&self, topic: &str) -> Result<(), CommError> {
        let mut inner = self.inner.write().await;
        let active = inner.active.as_ref().ok_or(CommError::NoActivePlugin)?;
        active.subscribe(topic).await?;

        if !inner.topics.contains(&topic.to_string()) {
            inner.topics.push(topic.to_string());
            info!(topic, "subscribed to topic");
        }
        Ok(())
    }

    /// Unsubscribe from a topic.
    pub async fn unsubscribe(&self, topic: &str) -> Result<(), CommError> {
        let mut inner = self.inner.write().await;
        let active = inner.active.as_ref().ok_or(CommError::NoActivePlugin)?;
        active.unsubscribe(topic).await?;

        inner.topics.retain(|t| t != topic);
        info!(topic, "unsubscribed from topic");
        Ok(())
    }

    /// Set the callback for incoming messages.
    pub async fn set_message_handler(&self, handler: MessageHandler) {
        let mut inner = self.inner.write().await;
        if let Some(ref active) = inner.active {
            active.set_message_handler(handler.clone());
        }
        inner.handler = Some(handler);
    }

    /// List subscribed topics.
    pub async fn list_topics(&self) -> Vec<String> {
        let inner = self.inner.read().await;
        inner.topics.clone()
    }

    /// List registered plugin names.
    pub async fn list_plugins(&self) -> Vec<String> {
        let inner = self.inner.read().await;
        inner.plugins.keys().cloned().collect()
    }

    /// Get manager status.
    pub async fn status(&self, agent_id: &str) -> ManagerStatus {
        let inner = self.inner.read().await;
        ManagerStatus {
            plugin_name: inner
                .active
                .as_ref()
                .map(|p| p.name().to_string())
                .unwrap_or_default(),
            connected: inner
                .active
                .as_ref()
                .map(|p| p.is_connected())
                .unwrap_or(false),
            topics: inner.topics.clone(),
            agent_id: agent_id.to_string(),
        }
    }

    /// Connect the active plugin with the given config.
    /// Uses snapshot-then-release: clone Arc, release lock, then do I/O.
    pub async fn connect_active(&self, config: HashMap<String, String>) -> Result<(), CommError> {
        let plugin = {
            let inner = self.inner.read().await;
            inner.active.clone().ok_or(CommError::NoActivePlugin)?
        };
        // Lock released — safe to do network I/O
        plugin.connect(config).await
    }

    /// Check if the active plugin is connected.
    pub async fn is_connected(&self) -> bool {
        let inner = self.inner.read().await;
        inner
            .active
            .as_ref()
            .map(|p| p.is_connected())
            .unwrap_or(false)
    }

    /// Retrieve and consume a rotated auth token from the active plugin.
    pub async fn take_rotated_token(&self) -> Option<String> {
        let inner = self.inner.read().await;
        if let Some(ref active) = inner.active {
            active.take_rotated_token().await
        } else {
            None
        }
    }

    /// Look up the agent slug for a conversation ID (agent_space detection).
    pub async fn agent_slug_for_conv(&self, conv_id: &str) -> Option<String> {
        let inner = self.inner.read().await;
        if let Some(ref active) = inner.active {
            active.agent_slug_for_conv(conv_id).await
        } else {
            None
        }
    }

    /// Disconnect all plugins.
    pub async fn shutdown(&self) {
        let mut inner = self.inner.write().await;
        for (name, plugin) in &inner.plugins {
            if plugin.is_connected() {
                if let Err(e) = plugin.disconnect().await {
                    warn!(plugin = name.as_str(), error = %e, "failed to disconnect plugin");
                }
            }
        }
        inner.active = None;
        inner.topics.clear();
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}
