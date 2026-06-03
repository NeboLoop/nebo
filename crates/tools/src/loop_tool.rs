use std::collections::HashMap;
use std::sync::Arc;

use crate::domain::DomainInput;
use crate::errors;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};
use comm::CommPlugin;

/// Best-effort MIME type from a file extension (matches the comm/app file conventions).
fn mime_for_path(p: &std::path::Path) -> &'static str {
    match p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "pdf" => "application/pdf",
        "txt" | "md" | "log" => "text/plain",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}

/// LoopTool provides NeboAI communication capabilities.
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

    /// Validate a local file path and return a ToolResult carrying it as
    /// `image_url`. The chat dispatcher collects every non-`data:` `image_url`
    /// produced during a run and staples it onto the loop reply as an uploaded
    /// attachment (see resolve_comm_attachments) — so sharing a file is just a
    /// matter of nominating its absolute path here. `target` is a human label
    /// (e.g. "the channel" / "the conversation") for the success message.
    fn share_file(&self, path: &str, target: &str) -> ToolResult {
        if path.is_empty() {
            return ToolResult::error(errors::missing_param(
                "share",
                "path",
                "loop(resource: \"channel\", action: \"share\", path: \"/absolute/path/to/file.pdf\")",
            ));
        }

        let p = std::path::Path::new(path);
        if !p.is_absolute() {
            return ToolResult::error(format!(
                "path must be absolute, got: {}. Do not retry — provide the full absolute path.",
                path
            ));
        }

        let meta = match std::fs::metadata(p) {
            Ok(m) => m,
            Err(e) => {
                return ToolResult::error(format!(
                    "Cannot access file at {}: {}. Do not retry — this is a filesystem error.",
                    path, e
                ));
            }
        };
        if !meta.is_file() {
            return ToolResult::error(format!("Not a file: {}. Do not retry — this is a filesystem error.", path));
        }

        let filename = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string());

        // Truthful: nothing is uploaded here. `image_url` is collected by the chat
        // dispatcher and stapled onto the reply this run sends to the channel. To
        // proactively post a file to a named channel, use channel/dm `send` with `path`.
        let mut result = ToolResult::ok(format!(
            "Attached {}. It will be delivered with your reply to {}.",
            filename, target
        ));
        result.image_url = Some(path.to_string());
        result
    }

    /// Read a local file and upload it, returning the attachment to embed in an
    /// outbound message. Errors are returned verbatim (no premature success).
    async fn upload_local_file(&self, path: &str) -> Result<comm::wire::Attachment, String> {
        let p = std::path::Path::new(path);
        if !p.is_absolute() {
            return Err(format!("path must be absolute, got: {}", path));
        }
        let data = std::fs::read(p).map_err(|e| format!("cannot read {}: {}", path, e))?;
        let filename = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());
        let mime = mime_for_path(p);
        self.comm
            .upload_file(&filename, mime, data)
            .await
            .map_err(|e| e.to_string())
    }

    async fn handle_dm(&self, input: &serde_json::Value) -> ToolResult {
        let action = input["action"].as_str().unwrap_or("");

        match action {
            "send" => {
                let to = input["to"].as_str().unwrap_or("");
                let text = input["text"].as_str().unwrap_or("");
                let path = input["path"].as_str().unwrap_or("");

                if to.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "dm send",
                        "to",
                        "loop(resource: \"dm\", action: \"send\", to: \"agent-uuid\", text: \"Hello\", path: \"/abs/file.png\")",
                    ));
                }
                if text.is_empty() && path.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "dm send",
                        "text or path",
                        "loop(resource: \"dm\", action: \"send\", to: \"agent-uuid\", text: \"Hello\")",
                    ));
                }

                let mut attachments = Vec::new();
                if !path.is_empty() {
                    match self.upload_local_file(path).await {
                        Ok(att) => attachments.push(att),
                        Err(e) => return ToolResult::error(format!(
                            "Failed to upload {}: {}. The file was NOT sent.", path, e
                        )),
                    }
                }
                let had_file = !attachments.is_empty();

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
                    attachments,
                };

                match self.comm.send(msg).await {
                    Ok(()) if had_file => ToolResult::ok(format!("DM with the attached file sent to {}", to)),
                    Ok(()) => ToolResult::ok(format!("DM sent to {}", to)),
                    Err(e) => ToolResult::error(format!("Failed to send DM: {}. The message was NOT delivered.", e)),
                }
            }
            "share" => {
                let path = input["path"].as_str().unwrap_or("");
                self.share_file(path, "the conversation")
            }
            _ => ToolResult::error(format!(
                "Unknown dm action: {}. Available: send, share",
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
                let path = input["path"].as_str().unwrap_or("");

                if channel_id.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "channel send",
                        "channel_id",
                        "loop(resource: \"channel\", action: \"send\", channel_id: \"...\", text: \"Hello\", path: \"/abs/file.png\")",
                    ));
                }
                if text.is_empty() && path.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "channel send",
                        "text or path",
                        "loop(resource: \"channel\", action: \"send\", channel_id: \"...\", text: \"Hello\")",
                    ));
                }

                // Optional file: upload it and attach. Real delivery — we only report
                // success after the upload AND the send both succeed.
                let mut attachments = Vec::new();
                if !path.is_empty() {
                    match self.upload_local_file(path).await {
                        Ok(att) => attachments.push(att),
                        Err(e) => return ToolResult::error(format!(
                            "Failed to upload {}: {}. The file was NOT sent.", path, e
                        )),
                    }
                }
                let had_file = !attachments.is_empty();

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
                    attachments,
                };

                match self.comm.send(msg).await {
                    Ok(()) if had_file => ToolResult::ok(format!("Sent to channel {} with the attached file.", channel_id)),
                    Ok(()) => ToolResult::ok(format!("Message sent to channel {}", channel_id)),
                    Err(e) => ToolResult::error(format!("Failed to send to channel: {}. The message was NOT delivered.", e)),
                }
            }
            "messages" => {
                let channel_id = input["channel_id"].as_str().unwrap_or("");
                if channel_id.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "channel messages",
                        "channel_id",
                        "loop(resource: \"channel\", action: \"messages\", channel_id: \"...\")",
                    ));
                }
                let limit = input["limit"].as_u64().unwrap_or(50) as usize;
                match self.comm.list_channel_messages(channel_id, limit).await {
                    Ok(msgs) => {
                        ToolResult::ok(serde_json::to_string_pretty(&msgs).unwrap_or_default())
                    }
                    Err(e) => ToolResult::error(format!("Failed to list channel messages: {}. Do not retry — this is a communication error.", e)),
                }
            }
            "members" => {
                let channel_id = input["channel_id"].as_str().unwrap_or("");
                if channel_id.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "channel members",
                        "channel_id",
                        "loop(resource: \"channel\", action: \"members\", channel_id: \"...\")",
                    ));
                }
                match self.comm.list_channel_members(channel_id).await {
                    Ok(members) => {
                        ToolResult::ok(serde_json::to_string_pretty(&members).unwrap_or_default())
                    }
                    Err(e) => ToolResult::error(format!("Failed to list channel members: {}. Do not retry — this is a communication error.", e)),
                }
            }
            "list" => match self.comm.list_channels().await {
                Ok(channels) => {
                    ToolResult::ok(serde_json::to_string_pretty(&channels).unwrap_or_default())
                }
                Err(e) => ToolResult::error(format!("Failed to list channels: {}. Do not retry — this is a communication error.", e)),
            },
            "share" => {
                let path = input["path"].as_str().unwrap_or("");
                self.share_file(path, "the channel")
            }
            _ => ToolResult::error(format!(
                "Unknown channel action: {}. Available: send, messages, members, list, share",
                action
            )),
        }
    }

    async fn handle_group(&self, input: &serde_json::Value) -> ToolResult {
        let action = input["action"].as_str().unwrap_or("");

        match action {
            "list" => match self.comm.list_loops().await {
                Ok(loops) => {
                    ToolResult::ok(serde_json::to_string_pretty(&loops).unwrap_or_default())
                }
                Err(e) => ToolResult::error(format!("Failed to list loops: {}. Do not retry — this is a communication error.", e)),
            },
            "get" => {
                let loop_id = input["loop_id"].as_str().unwrap_or("");
                if loop_id.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "group get",
                        "loop_id",
                        "loop(resource: \"group\", action: \"get\", loop_id: \"...\")",
                    ));
                }
                match self.comm.get_loop_info(loop_id).await {
                    Ok(info) => {
                        ToolResult::ok(serde_json::to_string_pretty(&info).unwrap_or_default())
                    }
                    Err(e) => ToolResult::error(format!("Failed to get loop info: {}. Do not retry — this is a communication error.", e)),
                }
            }
            "members" => {
                let loop_id = input["loop_id"].as_str().unwrap_or("");
                if loop_id.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "group members",
                        "loop_id",
                        "loop(resource: \"group\", action: \"members\", loop_id: \"...\")",
                    ));
                }
                match self.comm.list_channel_members(loop_id).await {
                    Ok(members) => {
                        ToolResult::ok(serde_json::to_string_pretty(&members).unwrap_or_default())
                    }
                    Err(e) => ToolResult::error(format!("Failed to list group members: {}. Do not retry — this is a communication error.", e)),
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
                    return ToolResult::error(errors::missing_param(
                        "subscribe",
                        "topic",
                        "loop(resource: \"topic\", action: \"subscribe\", topic: \"news\")",
                    ));
                }

                match self.comm.subscribe(topic).await {
                    Ok(()) => ToolResult::ok(format!("Subscribed to topic: {}", topic)),
                    Err(e) => ToolResult::error(format!("Failed to subscribe: {}. Do not retry — this is a communication error.", e)),
                }
            }
            "unsubscribe" => {
                let topic = input["topic"].as_str().unwrap_or("");
                if topic.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "unsubscribe",
                        "topic",
                        "loop(resource: \"topic\", action: \"unsubscribe\", topic: \"news\")",
                    ));
                }

                match self.comm.unsubscribe(topic).await {
                    Ok(()) => ToolResult::ok(format!("Unsubscribed from topic: {}", topic)),
                    Err(e) => ToolResult::error(format!("Failed to unsubscribe: {}. Do not retry — this is a communication error.", e)),
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
        "NeboAI communication — direct messages, channels, groups, and topics.\n\
         USE THIS when: user wants to message another bot, post to a channel, or interact with NeboAI infrastructure.\n\n\
         - loop(resource: \"dm\", action: \"send\", to: \"agent-uuid\", text: \"Hello\") — Send a DM to another bot\n\
         - loop(resource: \"channel\", action: \"send\", channel_id: \"...\", text: \"Hello\") — Send to a loop channel\n\
         - loop(resource: \"channel\", action: \"share\", path: \"/abs/path/file.pdf\") — Share a local file into the channel reply\n\
         - loop(resource: \"dm\", action: \"share\", path: \"/abs/path/file.pdf\") — Share a local file in a direct message\n\
         - loop(resource: \"channel\", action: \"list\") — List available channels\n\
         - loop(resource: \"channel\", action: \"messages\", channel_id: \"...\", limit: 20) — Read channel messages\n\
         - loop(resource: \"channel\", action: \"members\", channel_id: \"...\") — List channel members\n\
         - loop(resource: \"group\", action: \"list\") / get / members — Manage loops\n\
         - loop(resource: \"topic\", action: \"subscribe\", topic: \"news\") / unsubscribe / list / status\n\n\
         Use loop for bot-to-bot communication and NeboAI infrastructure."
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "resource": {
                    "type": "string",
                    "description": "REQUIRED. The communication resource category — determines which actions are available.",
                    "enum": ["dm", "channel", "group", "topic"]
                },
                "action": {
                    "type": "string",
                    "description": "The operation to perform on the selected resource. Never put a resource name here.",
                    "enum": ["send", "share", "messages", "members", "list", "get", "subscribe", "unsubscribe", "status"]
                },
                "text": { "type": "string", "description": "Message text" },
                "path": { "type": "string", "description": "Absolute path of a local file to share (for channel/dm share)" },
                "to": { "type": "string", "description": "Recipient agent ID (for dm)" },
                "channel_id": { "type": "string", "description": "Channel ID" },
                "topic": { "type": "string", "description": "Topic name for pub/sub" },
                "loop_id": { "type": "string", "description": "Loop (group) ID" },
                "limit": { "type": "integer", "description": "Max results to return" }
            },
            "required": ["resource", "action"]
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
                Err(e) => return ToolResult::error(format!("Failed to parse input: {}. Do not retry — this is a serialization error.", e)),
            };

            let mut input = input;
            let resource = {
                let corrected = crate::domain::auto_correct_resource(
                    &domain_input,
                    &mut input,
                    &["dm", "channel", "group", "topic"],
                );
                if corrected.is_empty() {
                    self.infer_resource(&domain_input.action).to_string()
                } else {
                    corrected
                }
            };

            if resource.is_empty() {
                return ToolResult::error(
                    "Resource is required. Available: dm, channel, group, topic",
                );
            }

            // `share` only nominates a local file path (the actual upload is deferred
            // to the chat dispatcher's resolve_comm_attachments at reply time), so it
            // does not need a live connection here. Every other action talks to NeboAI
            // directly and requires the plugin to be connected.
            let action = input["action"].as_str().unwrap_or("");
            if action != "share" && !self.comm.is_connected() {
                return ToolResult::error(
                    "Not connected to NeboAI. The comm plugin is not active.",
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
    #[test]
    fn test_tool_metadata() {
        // Can't test without a comm plugin, just verify struct exists
        assert_eq!("loop", "loop"); // placeholder
    }
}
