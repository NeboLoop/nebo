//! Adapters that bridge napp gRPC clients to internal tool/provider traits.
//!
//! napp crate generates gRPC client stubs but has no internal crate deps.
//! Server is the integration layer that already depends on napp, tools, and ai.

use std::sync::Arc;

use tokio::sync::RwLock;
use tonic::transport::Channel;

use napp::pb::{
    tool_service_client::ToolServiceClient,
    Empty, ExecuteRequest,
};
use tools::registry::DynTool;

/// Wraps a gRPC ToolServiceClient, implements tools::DynTool.
pub struct NappToolAdapter {
    tool_id: String,
    name: String,
    description: String,
    schema: serde_json::Value,
    requires_approval: bool,
    client: Arc<RwLock<ToolServiceClient<Channel>>>,
}

impl NappToolAdapter {
    pub async fn new(endpoint: &str, tool_id: String) -> Result<Self, String> {
        let channel = Channel::from_shared(endpoint.to_string())
            .map_err(|e| format!("invalid endpoint: {}", e))?
            .connect()
            .await
            .map_err(|e| format!("connect failed: {}", e))?;

        let mut client = ToolServiceClient::new(channel.clone());

        // Fetch metadata from the tool
        let name = client
            .name(Empty {})
            .await
            .map(|r| r.into_inner().name)
            .unwrap_or_else(|_| tool_id.clone());

        let description = client
            .description(Empty {})
            .await
            .map(|r| r.into_inner().description)
            .unwrap_or_default();

        let schema = client
            .schema(Empty {})
            .await
            .map(|r| {
                serde_json::from_slice(&r.into_inner().schema)
                    .unwrap_or_else(|_| serde_json::json!({"type": "object", "properties": {}}))
            })
            .unwrap_or_else(|_| serde_json::json!({"type": "object", "properties": {}}));

        let requires_approval = client
            .requires_approval(Empty {})
            .await
            .map(|r| r.into_inner().requires_approval)
            .unwrap_or(true);

        Ok(Self {
            tool_id,
            name,
            description,
            schema,
            requires_approval,
            client: Arc::new(RwLock::new(client)),
        })
    }
}

impl DynTool for NappToolAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    fn schema(&self) -> serde_json::Value {
        self.schema.clone()
    }

    fn requires_approval(&self) -> bool {
        self.requires_approval
    }

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a tools::ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = tools::ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let input_bytes = serde_json::to_vec(&input).unwrap_or_default();
            let mut client = self.client.write().await;
            match client.execute(ExecuteRequest { input: input_bytes }).await {
                Ok(resp) => {
                    let r = resp.into_inner();
                    if r.is_error {
                        tools::ToolResult::error(r.content)
                    } else {
                        tools::ToolResult::ok(r.content)
                    }
                }
                Err(e) => tools::ToolResult::error(format!(
                    "napp tool {} call failed: {}",
                    self.tool_id, e
                )),
            }
        })
    }
}
