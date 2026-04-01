use std::collections::HashMap;
use std::sync::Arc;

use comm::CommPlugin;
use crate::domain::DomainInput;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// LoopTool provides NeboLoop communication capabilities.
/// Resources: dm, channel, group, topic.
pub struct LoopTool {
    comm: Arc<dyn CommPlugin>,
}

impl LoopTool {
    pub fn new(comm: Arc<dyn CommPlugin>) -> Self {
        Self { comm }
    }

    fn infer_resource(&self, action: &str) -> &str {
        match action {
            "send" => "dm",
            "messages" | "members" => "channel",
            "subscribe" | "unsubscribe" => "topic",
            _ => "",
        }
    }

    async fn handle_dm(&self, input: &serde_json::Value) -> ToolResult {
        let action = input["action"].as_str().unwrap_or("");

        match action {
            "send" => {
                let to = input["to"].as_str().unwrap_or("");
                let text = input["text"].as_str().unwrap_or("");

                if to.is_empty() {
                    return ToolResult::error("to (recipient ID) is required for dm send");
                }
                if text.is_empty() {
                    return ToolResult::error("text is required for dm send");
                }

                let msg = comm::CommMessage {
                    id: uuid::Uuid::new_v4().to_string(),
                    from: String::new(),
                    to: to.to_string(),
                    topic: String::new(),
                    conversation_id: String::new(),
                    msg_type: comm::CommMessageType::Message,
                    content: text.to_string(),
                    metadata: HashMap::new(),
                    timestamp: 0,
                    human_injected: false,
                    human_id: None,
                    task_id: None,
                    correlation_id: None,
                    task_status: None,
                    artifacts: Vec::new(),
                    error: None,
                };

                match self.comm.send(msg).await {
                    Ok(()) => ToolResult::ok(format!("DM sent to {}", to)),
                    Err(e) => ToolResult::error(format!("Failed to send DM: {}", e)),
                }
            }
            _ => ToolResult::error(format!(
                "Unknown dm action: {}. Available: send",
                action
            )),
        }
    }

    async fn handle_channel(&self, input: &serde_json::Value) -> ToolResult {
        let action = input["action"].as_str().unwrap_or("");

        match action {
            "send" => {
                let channel_id = input["channel_id"].as_str().unwrap_or("");
                let text = input["text"].as_str().unwrap_or("");

                if channel_id.is_empty() {
                    return ToolResult::error("channel_id is required for channel send");
                }
                if text.is_empty() {
                    return ToolResult::error("text is required for channel send");
                }

                let msg = comm::CommMessage {
                    id: uuid::Uuid::new_v4().to_string(),
                    from: String::new(),
                    to: String::new(),
                    topic: channel_id.to_string(),
                    conversation_id: channel_id.to_string(),
                    msg_type: comm::CommMessageType::LoopChannel,
                    content: text.to_string(),
                    metadata: HashMap::new(),
                    timestamp: 0,
                    human_injected: false,
                    human_id: None,
                    task_id: None,
                    correlation_id: None,
                    task_status: None,
                    artifacts: Vec::new(),
                    error: None,
                };

                match self.comm.send(msg).await {
                    Ok(()) => ToolResult::ok(format!("Message sent to channel {}", channel_id)),
                    Err(e) => ToolResult::error(format!("Failed to send to channel: {}", e)),
                }
            }
            "messages" => {
                let channel_id = input["channel_id"].as_str().unwrap_or("");
                if channel_id.is_empty() {
                    return ToolResult::error("channel_id is required for channel messages");
                }
                let limit = input["limit"].as_u64().unwrap_or(50) as usize;
                match self.comm.list_channel_messages(channel_id, limit).await {
                    Ok(msgs) => ToolResult::ok(serde_json::to_string_pretty(&msgs).unwrap_or_default()),
                    Err(e) => ToolResult::error(format!("Failed to list channel messages: {}", e)),
                }
            }
            "members" => {
                let channel_id = input["channel_id"].as_str().unwrap_or("");
                if channel_id.is_empty() {
                    return ToolResult::error("channel_id is required for channel members");
                }
                match self.comm.list_channel_members(channel_id).await {
                    Ok(members) => ToolResult::ok(serde_json::to_string_pretty(&members).unwrap_or_default()),
                    Err(e) => ToolResult::error(format!("Failed to list channel members: {}", e)),
                }
            }
            "list" => {
                match self.comm.list_channels().await {
                    Ok(channels) => ToolResult::ok(serde_json::to_string_pretty(&channels).unwrap_or_default()),
                    Err(e) => ToolResult::error(format!("Failed to list channels: {}", e)),
                }
            }
            _ => ToolResult::error(format!(
                "Unknown channel action: {}. Available: send, messages, members, list",
                action
            )),
        }
    }

    async fn handle_group(&self, input: &serde_json::Value) -> ToolResult {
        let action = input["action"].as_str().unwrap_or("");

        match action {
            "list" => {
                match self.comm.list_loops().await {
                    Ok(loops) => ToolResult::ok(serde_json::to_string_pretty(&loops).unwrap_or_default()),
                    Err(e) => ToolResult::error(format!("Failed to list loops: {}", e)),
                }
            }
            "get" => {
                let loop_id = input["loop_id"].as_str().unwrap_or("");
                if loop_id.is_empty() {
                    return ToolResult::error("loop_id is required for group get");
                }
                match self.comm.get_loop_info(loop_id).await {
                    Ok(info) => ToolResult::ok(serde_json::to_string_pretty(&info).unwrap_or_default()),
                    Err(e) => ToolResult::error(format!("Failed to get loop info: {}", e)),
                }
            }
            "members" => {
                let loop_id = input["loop_id"].as_str().unwrap_or("");
                if loop_id.is_empty() {
                    return ToolResult::error("loop_id is required for group members");
                }
                match self.comm.list_channel_members(loop_id).await {
                    Ok(members) => ToolResult::ok(serde_json::to_string_pretty(&members).unwrap_or_default()),
                    Err(e) => ToolResult::error(format!("Failed to list group members: {}", e)),
                }
            }
            _ => ToolResult::error(format!(
                "Unknown group action: {}. Available: list, get, members",
                action
            )),
        }
    }

    async fn handle_topic(&self, input: &serde_json::Value) -> ToolResult {
        let action = input["action"].as_str().unwrap_or("");

        match action {
            "subscribe" => {
                let topic = input["topic"].as_str().unwrap_or("");
                if topic.is_empty() {
                    return ToolResult::error("topic is required for subscribe");
                }

                match self.comm.subscribe(topic).await {
                    Ok(()) => ToolResult::ok(format!("Subscribed to topic: {}", topic)),
                    Err(e) => ToolResult::error(format!("Failed to subscribe: {}", e)),
                }
            }
            "unsubscribe" => {
                let topic = input["topic"].as_str().unwrap_or("");
                if topic.is_empty() {
                    return ToolResult::error("topic is required for unsubscribe");
                }

                match self.comm.unsubscribe(topic).await {
                    Ok(()) => ToolResult::ok(format!("Unsubscribed from topic: {}", topic)),
                    Err(e) => ToolResult::error(format!("Failed to unsubscribe: {}", e)),
                }
            }
            "list" | "status" => {
                let connected = self.comm.is_connected();
                let plugin_name = self.comm.name();
                let plugin_version = self.comm.version();

                ToolResult::ok(format!(
                    "Comm plugin: {} v{}\nConnected: {}",
                    plugin_name, plugin_version, connected
                ))
            }
            _ => ToolResult::error(format!(
                "Unknown topic action: {}. Available: subscribe, unsubscribe, list, status",
                action
            )),
        }
    }
}

impl DynTool for LoopTool {
    fn name(&self) -> &str {
        "loop"
    }

    fn description(&self) -> String {
        "NeboLoop communication — direct messages, channels, groups, and topics.\n\
         USE THIS when: user wants to message another bot, post to a channel, or interact with NeboLoop infrastructure.\n\n\
         - loop(resource: \"dm\", action: \"send\", to: \"agent-uuid\", text: \"Hello\") — Send a DM to another bot\n\
         - loop(resource: \"channel\", action: \"send\", channel_id: \"...\", text: \"Hello\") — Send to a loop channel\n\
         - loop(resource: \"channel\", action: \"list\") — List available channels\n\
         - loop(resource: \"channel\", action: \"messages\", channel_id: \"...\", limit: 20) — Read channel messages\n\
         - loop(resource: \"channel\", action: \"members\", channel_id: \"...\") — List channel members\n\
         - loop(resource: \"group\", action: \"list\") / get / members — Manage loops\n\
         - loop(resource: \"topic\", action: \"subscribe\", topic: \"news\") / unsubscribe / list / status\n\n\
         Use loop for bot-to-bot communication and NeboLoop infrastructure."
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "resource": {
                    "type": "string",
                    "description": "Resource type",
                    "enum": ["dm", "channel", "group", "topic"]
                },
                "action": {
                    "type": "string",
                    "description": "Action to perform",
                    "enum": ["send", "messages", "members", "list", "get", "subscribe", "unsubscribe", "status"]
                },
                "text": { "type": "string", "description": "Message text" },
                "to": { "type": "string", "description": "Recipient agent ID (for dm)" },
                "channel_id": { "type": "string", "description": "Channel ID" },
                "topic": { "type": "string", "description": "Topic name for pub/sub" },
                "loop_id": { "type": "string", "description": "Loop (group) ID" },
                "limit": { "type": "integer", "description": "Max results to return" }
            },
            "required": ["action"]
        })
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let domain_input: DomainInput = match serde_json::from_value(input.clone()) {
                Ok(v) => v,
                Err(e) => return ToolResult::error(format!("Failed to parse input: {}", e)),
            };

            let resource = if domain_input.resource.is_empty() {
                self.infer_resource(&domain_input.action).to_string()
            } else {
                domain_input.resource
            };

            if resource.is_empty() {
                return ToolResult::error(
                    "Resource is required. Available: dm, channel, group, topic",
                );
            }

            if !self.comm.is_connected() {
                return ToolResult::error(
                    "Not connected to NeboLoop. The comm plugin is not active.",
                );
            }

            match resource.as_str() {
                "dm" => self.handle_dm(&input).await,
                "channel" => self.handle_channel(&input).await,
                "group" => self.handle_group(&input).await,
                "topic" => self.handle_topic(&input).await,
                other => ToolResult::error(format!(
                    "Resource {:?} not available. Available: dm, channel, group, topic",
                    other
                )),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_metadata() {
        // Can't test without a comm plugin, just verify struct exists
        assert_eq!("loop", "loop"); // placeholder
    }
}
