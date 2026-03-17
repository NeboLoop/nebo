use std::sync::Arc;

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// McpTool is a STRAP domain tool for calling connected MCP servers.
/// Usage: mcp(server: "monument.sh", resource: "project", action: "list")
pub struct McpTool {
    bridge: Arc<mcp::Bridge>,
    store: Arc<db::Store>,
}

impl McpTool {
    pub fn new(bridge: Arc<mcp::Bridge>, store: Arc<db::Store>) -> Self {
        Self { bridge, store }
    }

    /// Build a dynamic description showing connected servers and their tools.
    fn build_description(&self) -> String {
        let mut desc = String::from(
            "Call tools on connected MCP servers.\n\n\
             Resources (server name) and Actions (tool name) depend on which servers are connected.\n\n"
        );

        // List connected servers and their tools from the bridge
        let connected = self.bridge.connected_tools();
        if connected.is_empty() {
            desc.push_str("No MCP servers currently connected. Add servers in Connectors settings.\n");
        } else {
            desc.push_str("Connected servers:\n");
            for (server, tools) in &connected {
                let display = server.replace('_', ".");
                desc.push_str(&format!("- {} → {}\n", display, tools.join(", ")));
            }
            desc.push_str("\nExamples:\n");
            if let Some((server, tools)) = connected.first() {
                let display = server.replace('_', ".");
                if let Some(tool) = tools.first() {
                    desc.push_str(&format!(
                        "  mcp(server: \"{}\", resource: \"{}\", action: \"list\")\n",
                        display, tool
                    ));
                }
            }
        }

        desc
    }
}

impl DynTool for McpTool {
    fn name(&self) -> &str {
        "mcp"
    }

    fn description(&self) -> String {
        self.build_description()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "server": {
                    "type": "string",
                    "description": "MCP server name (e.g., \"monument.sh\")"
                },
                "resource": {
                    "type": "string",
                    "description": "Tool/resource name on the server (e.g., \"project\", \"todo\")"
                },
                "action": {
                    "type": "string",
                    "description": "Action to perform (e.g., \"list\", \"create\", \"get\")"
                }
            },
            "required": ["server", "resource"],
            "additionalProperties": true
        })
    }

    fn requires_approval(&self) -> bool {
        true
    }

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let server = match input.get("server").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => return ToolResult::error("server is required. Specify the MCP server name."),
            };
            let resource = match input.get("resource").and_then(|v| v.as_str()) {
                Some(r) => r,
                None => return ToolResult::error("resource is required. Specify the tool name on the server."),
            };

            // Slugify server name to match the bridge key
            let server_slug = server
                .to_lowercase()
                .chars()
                .map(|c| if c.is_alphanumeric() { c } else { '_' })
                .collect::<String>();
            let server_slug = server_slug.trim_matches('_');

            // Find the integration by matching slug against connected tools
            let connected = self.bridge.connected_tools();
            let (integration_id, _tools) = match connected.iter().find(|(s, _)| {
                s == server_slug || s.contains(server_slug)
            }) {
                Some((_, tools)) => {
                    // Find integration ID from bridge connections
                    match self.bridge.find_integration_for_tool(server_slug, resource) {
                        Some(id) => (id, tools),
                        None => return ToolResult::error(format!(
                            "Server '{}' is connected but tool '{}' not found. Available tools: {}",
                            server, resource, tools.join(", ")
                        )),
                    }
                }
                None => {
                    let available: Vec<String> = connected.iter()
                        .map(|(s, _)| s.replace('_', "."))
                        .collect();
                    return ToolResult::error(format!(
                        "MCP server '{}' is not connected. Connected servers: {}",
                        server,
                        if available.is_empty() { "none".to_string() } else { available.join(", ") }
                    ));
                }
            };

            // Build the input for the MCP call — pass everything except server/resource
            let mut mcp_input = input.clone();
            if let Some(obj) = mcp_input.as_object_mut() {
                obj.remove("server");
                obj.remove("resource");
            }

            // Call the tool via bridge
            match self.bridge.call_tool(&integration_id, resource, mcp_input).await {
                Ok(result) => {
                    if result.is_error {
                        ToolResult::error(result.content)
                    } else {
                        ToolResult::ok(result.content)
                    }
                }
                Err(e) => ToolResult::error(format!("MCP call failed: {}", e)),
            }
        })
    }
}
