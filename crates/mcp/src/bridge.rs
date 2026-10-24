use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::client::McpClient;
use crate::{McpError, McpToolDef, McpToolResult};

/// Tracks a live connection to an external MCP server.
struct Connection {
    integration_id: String,
    server_slug: String,
    tool_names: Vec<String>,     // namespaced: mcp__server__tool
    original_names: Vec<String>, // original tool names from the server
    /// The server's definitions, as it offered them when last listed.
    defs: Vec<McpToolDef>,
}

/// A server's tools listed again on its live connection.
#[derive(Debug, Clone)]
pub struct Relisted {
    /// Every tool the server offers now.
    pub tools: Vec<McpToolDef>,
    /// The tools (their own names) that arrived, left, or changed definition.
    pub changed: Vec<String>,
}

/// The tools (their own names) that arrived, left, or changed definition
/// between two listings of one server.
pub fn changed_tools(before: &[McpToolDef], after: &[McpToolDef]) -> Vec<String> {
    let value = |d: &McpToolDef| serde_json::to_value(d).unwrap_or_default();
    let mut changed: Vec<String> = after
        .iter()
        .filter(|a| before.iter().find(|b| b.name == a.name).is_none_or(|b| value(b) != value(a)))
        .map(|a| a.name.clone())
        .collect();
    changed.extend(
        before
            .iter()
            .filter(|b| !after.iter().any(|a| a.name == b.name))
            .map(|b| b.name.clone()),
    );
    changed
}

/// Callback to register/unregister proxy tools in the agent's tool registry.
pub trait ProxyToolRegistry: Send + Sync {
    /// Register `tool` under its proxy `name` (`mcp__<server>__<tool>`).
    fn register_proxy(&self, name: &str, tool: &McpToolDef, integration_id: &str);
    fn unregister_proxy(&self, name: &str);
    /// Called after a connect finishes registering a server's tools — the ONE
    /// hook where the server's tools get their permission rules: the
    /// server's default (they ask until the owner says otherwise), and an
    /// ask on each tool the server never offered before while its default
    /// allows. `server_slug` is the tool-name prefix
    /// (`mcp__<server_slug>__<tool>`); `tools` pairs each tool's own name
    /// with its proxy name.
    fn tools_synced(&self, integration_id: &str, server_slug: &str, tools: &[(String, String)]);
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
        // arguments. This is the single canonical call pathway for MCP tools.
        let tool_names: Vec<String> = tools
            .iter()
            .map(|t| make_tool_name(server_type, &t.name))
            .collect();
        let original_names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();

        for (t, proxy_name) in tools.iter().zip(tool_names.iter()) {
            self.registry.register_proxy(proxy_name, t, integration_id);
        }

        // Every connect IS the tool sync (startup reconnect, settings connect,
        // OAuth callback, refresh): the server's tools get their default rule.
        let synced: Vec<(String, String)> = original_names
            .iter()
            .cloned()
            .zip(tool_names.iter().cloned())
            .collect();
        self.registry
            .tools_synced(integration_id, &server_slug(server_type), &synced);

        let mut conns = self.connections.lock().await;
        conns.insert(
            integration_id.to_string(),
            Connection {
                integration_id: integration_id.to_string(),
                server_slug: server_type.to_string(),
                tool_names,
                original_names,
                defs: tools.clone(),
            },
        );

        info!(
            server_type,
            tools = tools.len(),
            "connected MCP integration"
        );
        Ok(tools)
    }

    /// List a connected server's tools again on its live connection (no
    /// reconnect: a stdio server keeps its process) and register what
    /// changed: a new or changed tool is registered with its new definition,
    /// a tool the server stopped offering is removed. Every chat reads its
    /// tools from the registry, so the next step of a running chat and every
    /// new chat get the new definitions.
    pub async fn refresh(&self, integration_id: &str) -> Result<Relisted, McpError> {
        let (server_type, before) = {
            let conns = self.connections.lock().await;
            let conn = conns
                .get(integration_id)
                .ok_or_else(|| McpError::NotFound(format!("connection {integration_id}")))?;
            (conn.server_slug.clone(), conn.defs.clone())
        };
        let tools = self.client.relist_tools(integration_id).await?;
        let changed = changed_tools(&before, &tools);

        let mut conns = self.connections.lock().await;
        let Some(conn) = conns.get_mut(integration_id) else {
            return Err(McpError::NotFound(format!("connection {integration_id}")));
        };
        for gone in before.iter().filter(|b| !tools.iter().any(|t| t.name == b.name)) {
            self.registry.unregister_proxy(&make_tool_name(&server_type, &gone.name));
        }
        let tool_names: Vec<String> = tools.iter().map(|t| make_tool_name(&server_type, &t.name)).collect();
        let original_names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        for (t, proxy_name) in tools.iter().zip(tool_names.iter()) {
            if changed.contains(&t.name) {
                self.registry.register_proxy(proxy_name, t, integration_id);
            }
        }
        if !changed.is_empty() {
            let synced: Vec<(String, String)> =
                original_names.iter().cloned().zip(tool_names.iter().cloned()).collect();
            self.registry.tools_synced(integration_id, &server_slug(&server_type), &synced);
        }
        conn.tool_names = tool_names;
        conn.original_names = original_names;
        conn.defs = tools.clone();
        info!(id = integration_id, tools = tools.len(), changed = changed.len(), "MCP tools listed again");
        Ok(Relisted { tools, changed })
    }

    /// List a server's tools again whenever it says they changed
    /// (`notifications/tools/list_changed`). Started once, beside the
    /// bridge; ends when the bridge is dropped.
    pub fn watch_tool_changes(self: &Arc<Self>) {
        let Some(mut changed) = self.client.take_tools_changed() else {
            return;
        };
        let bridge = Arc::downgrade(self);
        tokio::spawn(async move {
            while let Some(id) = changed.recv().await {
                let Some(bridge) = bridge.upgrade() else { break };
                if let Err(e) = bridge.refresh(&id).await {
                    warn!(integration = %id, error = %e, "MCP tools changed but could not be listed again");
                }
            }
        });
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

    /// The tools a connected server offers, as it defined them (their own
    /// names, titles and descriptions). Empty when it isn't connected.
    pub async fn tool_defs(&self, integration_id: &str) -> Vec<McpToolDef> {
        self.connections
            .lock()
            .await
            .get(integration_id)
            .map(|c| c.defs.clone())
            .unwrap_or_default()
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
        self.call_tool_scoped(integration_id, tool_name, input, None).await
    }

    /// Call inside a confidentiality scope; see `McpClient::call_tool_scoped`.
    pub async fn call_tool_scoped(
        &self,
        integration_id: &str,
        tool_name: &str,
        input: serde_json::Value,
        matter: Option<&str>,
    ) -> Result<McpToolResult, McpError> {
        self.client
            .call_tool_scoped(integration_id, tool_name, input, matter)
            .await
    }

    /// Read a view (`resources/read` of its `ui://` resource) from a
    /// connected server.
    pub async fn read_resource(&self, integration_id: &str, uri: &str) -> Result<crate::ui::UiResource, McpError> {
        self.client.read_resource(integration_id, uri).await
    }

    /// A connected server's tool by its own name, with the proxy name it is
    /// registered under (`mcp__<server>__<tool>`). `None` when the server
    /// isn't connected or offers no such tool: a view's call reaches only
    /// the tools of the server that showed it.
    pub async fn server_tool(&self, integration_id: &str, tool_name: &str) -> Option<(McpToolDef, String)> {
        let conns = self.connections.lock().await;
        let conn = conns.get(integration_id)?;
        let def = conn.defs.iter().find(|d| d.name == tool_name)?.clone();
        Some((def, make_tool_name(&conn.server_slug, tool_name)))
    }

    /// List connected integration IDs.
    pub async fn connected_ids(&self) -> Vec<String> {
        let conns = self.connections.lock().await;
        conns.keys().cloned().collect()
    }
}

/// Generate a namespaced tool name: mcp__{server_type}__{tool_name}, each
/// part lowercased with anything outside `[a-z0-9_-]` written as `_` (the
/// names providers accept).
pub fn make_tool_name(server_type: &str, original: &str) -> String {
    format!("mcp__{}__{}", server_slug(server_type), sanitize(original))
}

fn sanitize(part: &str) -> String {
    part.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect()
}

/// MCP tool-name prefix for an integration — e.g. "monument.sh" →
/// "monument_sh", "My GitHub" → "my_github".
///
/// DELIBERATELY not `comm::handle::slugify` (the routing-handle slugifier):
/// the underscore alphabet here is load-bearing — these prefixes are baked
/// into stored MCP tool names and permission rules, so the mapping must stay
/// byte-stable even though it doesn't collapse runs the way handle slugs do.
pub fn tool_name_prefix(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

/// The server part of its proxy tools' names (`mcp__<slug>__<tool>`).
pub fn server_slug(server_type: &str) -> String {
    sanitize(server_type)
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
        // Anything a provider would refuse in a name becomes `_`.
        assert_eq!(make_tool_name("docs.example", "files/read:v2"), "mcp__docs_example__files_read_v2");

    }

    /// A listing's change is every tool that arrived, left, or changed
    /// definition; an identical listing changes nothing.
    #[test]
    fn changed_tools_are_the_ones_that_arrived_left_or_changed() {
        let def = |v: serde_json::Value| serde_json::from_value::<McpToolDef>(v).unwrap();
        let before = vec![
            def(serde_json::json!({"name": "prices", "inputSchema": {"type": "object", "properties": {"ids": {"type": "array"}}}})),
            def(serde_json::json!({"name": "old"})),
            def(serde_json::json!({"name": "same", "description": "x"})),
        ];
        let after = vec![
            def(serde_json::json!({"name": "prices", "inputSchema": {"type": "object", "properties": {"ids": {"type": "array"}, "display_names": {"type": "boolean"}}}})),
            def(serde_json::json!({"name": "new"})),
            def(serde_json::json!({"name": "same", "description": "x"})),
        ];
        assert_eq!(changed_tools(&before, &after), vec!["prices", "new", "old"]);
        assert!(changed_tools(&after, &after).is_empty());
    }
}
