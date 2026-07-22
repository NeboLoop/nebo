use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::info;

use crate::client::McpClient;
use crate::{McpError, McpToolDef, McpToolResult};

/// Tracks a live connection to an external MCP server.
struct Connection {
    integration_id: String,
    server_slug: String,
    tool_names: Vec<String>,     // namespaced: mcp__server__tool
    original_names: Vec<String>, // original tool names from the server
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

/// Launch spec for a local stdio MCP server, parsed from an integration's
/// `metadata` JSON (`{ "command": "...", "args": [...], "env": { } }`) — the
/// stdio half of the standard MCP server config block.
struct StdioConfig {
    command: String,
    args: Vec<String>,
    env: HashMap<String, String>,
}

/// Parse the stdio launch spec from an integration's `metadata` JSON. Returns
/// None when there's no usable `command`.
fn parse_stdio_config(metadata: Option<&str>) -> Option<StdioConfig> {
    let v: serde_json::Value = serde_json::from_str(metadata?).ok()?;
    let command = v.get("command")?.as_str()?.to_string();
    if command.is_empty() {
        return None;
    }
    let args = v
        .get("args")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let env = v
        .get("env")
        .and_then(|e| e.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, val)| val.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    Some(StdioConfig { command, args, env })
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

    /// Get a reference to the underlying MCP client (for OAuth/encryption operations).
    pub fn client(&self) -> &McpClient {
        &self.client
    }

    /// Connect to a single MCP integration.
    pub async fn connect(
        &self,
        integration_id: &str,
        server_type: &str,
        server_url: &str,
        access_token: Option<&str>,
        metadata: Option<&str>,
    ) -> Result<Vec<McpToolDef>, McpError> {
        // Disconnect existing
        self.disconnect(integration_id).await;

        // Dispatch by transport. A stdio server carries a launch spec (command/
        // args/env) in metadata; a remote server has none and connects at
        // server_url. (`server_type` here is the tool-name prefix, not the
        // transport, so presence of a stdio spec is the authoritative signal.)
        let tools = if let Some(cfg) = parse_stdio_config(metadata) {
            self.client
                .connect_stdio(integration_id, &cfg.command, &cfg.args, &cfg.env)
                .await?
        } else {
            self.client
                .list_tools(integration_id, server_url, access_token)
                .await?
        };

        // Expose each external tool as its own proxy tool (`mcp__<server>__<tool>`)
        // carrying the server's real input schema, so the model calls it with correct
        // arguments. This is the single canonical call pathway for MCP tools — the `mcp`
        // STRAP tool itself only enumerates servers (mcp(action:"list")).
        let tool_names: Vec<String> = tools
            .iter()
            .map(|t| make_tool_name(server_type, &t.name))
            .collect();
        let original_names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();

        for (t, proxy_name) in tools.iter().zip(tool_names.iter()) {
            self.registry.register_proxy(
                proxy_name,
                &t.name,
                &t.description,
                t.input_schema.clone(),
                integration_id,
            );
        }

        let mut conns = self.connections.lock().await;
        conns.insert(
            integration_id.to_string(),
            Connection {
                integration_id: integration_id.to_string(),
                server_slug: server_type.to_string(),
                tool_names,
                original_names,
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

    /// List connected servers and their original tool names.
    /// Returns Vec<(server_slug, Vec<tool_name>)>.
    pub fn connected_tools(&self) -> Vec<(String, Vec<String>)> {
        // Use try_lock to avoid blocking — return empty if locked
        match self.connections.try_lock() {
            Ok(conns) => conns
                .values()
                .map(|c| (c.server_slug.clone(), c.original_names.clone()))
                .collect(),
            Err(_) => vec![],
        }
    }

    /// Find the integration ID for a server+tool combination.
    pub fn find_integration_for_tool(&self, server_slug: &str, tool_name: &str) -> Option<String> {
        match self.connections.try_lock() {
            Ok(conns) => conns
                .values()
                .find(|c| {
                    (c.server_slug == server_slug || c.server_slug.contains(server_slug))
                        && c.original_names.iter().any(|t| t == tool_name)
                })
                .map(|c| c.integration_id.clone()),
            Err(_) => None,
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
        self.client
            .call_tool(integration_id, tool_name, input)
            .await
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
