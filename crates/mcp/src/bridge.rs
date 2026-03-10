use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{error, info};

use crate::client::McpClient;
use crate::{McpError, McpToolDef, McpToolResult};

/// Tracks a live connection to an external MCP server.
struct Connection {
    _integration_id: String,
    _server_type: String,
    tool_names: Vec<String>, // namespaced names registered in the tool registry
}

/// Callback to register/unregister proxy tools in the agent's tool registry.
pub trait ProxyToolRegistry: Send + Sync {
    fn register_proxy(
        &self,
        name: &str,
        original_name: &str,
        description: &str,
        schema: Option<serde_json::Value>,
        integration_id: &str,
    );
    fn unregister_proxy(&self, name: &str);
}

/// Integration info needed by the bridge (from DB).
pub struct IntegrationInfo {
    pub id: String,
    pub name: String,
    pub server_type: String,
    pub server_url: Option<String>,
    pub auth_type: String,
    pub is_enabled: bool,
    pub connection_status: Option<String>,
}

/// Bridge manages connections to external MCP servers and registers their tools
/// as proxy tools in the agent's tool registry.
pub struct Bridge {
    connections: Mutex<HashMap<String, Connection>>,
    client: Arc<McpClient>,
    registry: Arc<dyn ProxyToolRegistry>,
}

impl Bridge {
    pub fn new(client: Arc<McpClient>, registry: Arc<dyn ProxyToolRegistry>) -> Self {
        Self {
            connections: Mutex::new(HashMap::new()),
            client,
            registry,
        }
    }

    /// Sync all enabled integrations. Disconnects stale, connects new.
    pub async fn sync_all(
        &self,
        integrations: &[IntegrationInfo],
    ) -> Result<(), McpError> {
        let enabled: HashMap<&str, &IntegrationInfo> = integrations
            .iter()
            .filter(|i| i.is_enabled)
            .map(|i| (i.id.as_str(), i))
            .collect();

        // Disconnect stale
        {
            let mut conns = self.connections.lock().await;
            let stale: Vec<String> = conns
                .keys()
                .filter(|id| !enabled.contains_key(id.as_str()))
                .cloned()
                .collect();
            for id in stale {
                self.disconnect_locked(&mut conns, &id).await;
            }
        }

        // Connect new/updated
        let mut last_err = None;
        for info in integrations.iter().filter(|i| i.is_enabled) {
            let server_url = match &info.server_url {
                Some(url) if !url.is_empty() => url.clone(),
                _ => continue,
            };

            // Skip OAuth integrations without completed auth
            if info.auth_type == "oauth" && info.connection_status.is_none() {
                continue;
            }

            if let Err(e) = self.connect(&info.id, &info.server_type, &server_url, None).await {
                error!(name = info.name.as_str(), id = info.id.as_str(), error = %e, "failed to connect integration");
                last_err = Some(e);
            }
        }

        match last_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Connect to a single MCP integration.
    pub async fn connect(
        &self,
        integration_id: &str,
        server_type: &str,
        server_url: &str,
        access_token: Option<&str>,
    ) -> Result<Vec<McpToolDef>, McpError> {
        // Disconnect existing
        self.disconnect(integration_id).await;

        // List tools from external server
        let tools = self
            .client
            .list_tools(integration_id, server_url, access_token)
            .await?;

        // Register each tool as a proxy
        let mut tool_names = Vec::with_capacity(tools.len());
        for tool in &tools {
            let proxy_name = make_tool_name(server_type, &tool.name);
            self.registry.register_proxy(
                &proxy_name,
                &tool.name,
                &tool.description,
                tool.input_schema.clone(),
                integration_id,
            );
            tool_names.push(proxy_name);
        }

        let mut conns = self.connections.lock().await;
        conns.insert(
            integration_id.to_string(),
            Connection {
                _integration_id: integration_id.to_string(),
                _server_type: server_type.to_string(),
                tool_names,
            },
        );

        info!(
            server_type,
            tools = tools.len(),
            "connected MCP integration"
        );
        Ok(tools)
    }

    /// Disconnect an integration and remove its proxy tools.
    pub async fn disconnect(&self, integration_id: &str) {
        let mut conns = self.connections.lock().await;
        self.disconnect_locked(&mut conns, integration_id).await;
    }

    async fn disconnect_locked(
        &self,
        conns: &mut HashMap<String, Connection>,
        integration_id: &str,
    ) {
        if let Some(conn) = conns.remove(integration_id) {
            for name in &conn.tool_names {
                self.registry.unregister_proxy(name);
            }
            self.client.close_session(integration_id).await;
            info!(
                id = integration_id,
                tools = conn.tool_names.len(),
                "disconnected MCP integration"
            );
        }
    }

    /// Close all connections.
    pub async fn close(&self) {
        let mut conns = self.connections.lock().await;
        let ids: Vec<String> = conns.keys().cloned().collect();
        for id in ids {
            self.disconnect_locked(&mut conns, &id).await;
        }
    }

    /// Call a tool on a connected integration.
    pub async fn call_tool(
        &self,
        integration_id: &str,
        tool_name: &str,
        input: serde_json::Value,
    ) -> Result<McpToolResult, McpError> {
        self.client.call_tool(integration_id, tool_name, input).await
    }

    /// List connected integration IDs.
    pub async fn connected_ids(&self) -> Vec<String> {
        let conns = self.connections.lock().await;
        conns.keys().cloned().collect()
    }
}

/// Generate a namespaced tool name: mcp__{server_type}__{tool_name}
fn make_tool_name(server_type: &str, original: &str) -> String {
    let st = server_type.to_lowercase().replace(' ', "_");
    let tn = original.to_lowercase().replace(' ', "_");
    format!("mcp__{}__{}", st, tn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_name() {
        assert_eq!(
            make_tool_name("brave-search", "web_search"),
            "mcp__brave-search__web_search"
        );
        assert_eq!(
            make_tool_name("My Server", "do_thing"),
            "mcp__my_server__do_thing"
        );
        // Tool names with spaces and mixed case get normalized
        assert_eq!(
            make_tool_name("slack", "Send Message"),
            "mcp__slack__send_message"
        );
    }
}
