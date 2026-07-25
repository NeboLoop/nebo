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
/// Resources: dm, channel, loop, topic.
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

    /// Resolve employee names/slugs to mention tokens for a channel.
    /// Agents (AI employees — including other bots' agents) resolve to
    /// `<@loop_agent_id>`; bare bot names fall back to `<@bot_id>` (routes to
    /// that bot's primary employee). Returns (tokens, unresolved_names).
    async fn resolve_mentions(
        &self,
        channel_id: &str,
        names: &[String],
    ) -> (Vec<String>, Vec<String>) {
        // Channel → loop mapping comes from the bot's channel list.
        let loop_id = match self.comm.list_channels().await {
            Ok(channels) => channels
                .into_iter()
                .find(|c| c.channel_id == channel_id)
                .map(|c| c.loop_id),
            Err(_) => None,
        };
        let agents = match &loop_id {
            Some(lid) => self.comm.list_loop_agents(lid).await.unwrap_or_default(),
            None => Vec::new(),
        };
        let members = self
            .comm
            .list_channel_members(channel_id)
            .await
            .unwrap_or_default();

        let mut tokens = Vec::new();
        let mut unresolved = Vec::new();
        for name in names {
            let needle = name.to_lowercase();
            // Employees first (agents within bots), then bot-level fallback.
            // A bare bot name ("Alpha") resolves to that bot's agent too —
            // agent rows carry the hosting bot's name/slug.
            let agent = agents.iter().find(|a| {
                a.name.to_lowercase() == needle
                    || a.slug.to_lowercase() == needle
                    || a.bot_name.to_lowercase() == needle
                    || a.bot_slug.to_lowercase() == needle
            });
            if let Some(a) = agent {
                // The gateway manages each bot's PRIMARY agent under the BOT id
                // (its row has the bare `bot_<id8>` slug and no loop_agent_id on
                // the receiving side) — mention it as <@bot_id>, matching what
                // the web picker emits. Named secondaries use <@loop_agent_id>.
                let bot_id_hex = a.bot_id.replace('-', "");
                let is_primary = bot_id_hex.len() >= 8
                    && a.slug == format!("bot_{}", &bot_id_hex[..8]);
                let token = if is_primary {
                    format!("<@{}>", a.bot_id)
                } else {
                    format!("<@{}>", a.id)
                };
                if !tokens.contains(&token) {
                    tokens.push(token);
                }
                continue;
            }
            let member = members.iter().find(|m| {
                m.bot_name.to_lowercase() == needle
            });
            if let Some(m) = member {
                let token = format!("<@{}>", m.bot_id);
                if !tokens.contains(&token) {
                    tokens.push(token);
                }
                continue;
            }
            unresolved.push(name.clone());
        }
        (tokens, unresolved)
    }

    async fn handle_dm(&self, input: &serde_json::Value, handoff_depth: u8) -> ToolResult {
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

                // Tag agent-authored DMs so receiving bots apply handoff guardrails.
                let mut metadata = HashMap::new();
                metadata.insert("senderKind".to_string(), "agent".to_string());
                if handoff_depth > 0 {
                    metadata.insert("handoffDepth".to_string(), handoff_depth.to_string());
                }
                let msg = comm::CommMessage {
                    id: uuid::Uuid::new_v4().to_string(),
                    from: String::new(),
                    to: to.to_string(),
                    topic: String::new(),
                    conversation_id: String::new(),
                    msg_type: comm::CommMessageType::Message,
                    content: text.to_string(),
                    metadata,
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

    async fn handle_channel(&self, input: &serde_json::Value, handoff_depth: u8) -> ToolResult {
        let action = input["action"].as_str().unwrap_or("");

        match action {
            "ensure" => {
                let name = input["name"].as_str().unwrap_or("");
                if name.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "channel ensure",
                        "name",
                        "loop(resource: \"channel\", action: \"ensure\", name: \"daily-briefing\")",
                    ));
                }
                let description = input["description"].as_str().filter(|s| !s.is_empty());
                match self.comm.ensure_channel(name, description).await {
                    Ok(channel_id) => ToolResult::ok(format!(
                        "Channel \"{}\" is ready (channel_id: {}). Post to it with \
                         loop(resource: \"channel\", action: \"send\", channel_id: \"{}\", text: \"...\").",
                        name, channel_id, channel_id
                    )),
                    Err(e) => {
                        ToolResult::error(format!("Failed to ensure channel \"{}\": {}", name, e))
                    }
                }
            }
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

                // Optional `mention`: employee names/slugs (string or array) to
                // hand this message to. Resolved to `<@id>` tokens the loop's
                // mention routing understands, prepended to the text — the
                // mentioned employees' bots pick the message up and run.
                let mention_names: Vec<String> = match &input["mention"] {
                    serde_json::Value::String(s) => s
                        .split(',')
                        .map(|p| p.trim().trim_start_matches('@').to_string())
                        .filter(|p| !p.is_empty())
                        .collect(),
                    serde_json::Value::Array(items) => items
                        .iter()
                        .filter_map(|v| v.as_str())
                        .map(|p| p.trim().trim_start_matches('@').to_string())
                        .filter(|p| !p.is_empty())
                        .collect(),
                    _ => Vec::new(),
                };
                let mut mention_tokens: Vec<String> = Vec::new();
                let mut unresolved: Vec<String> = Vec::new();
                if !mention_names.is_empty() {
                    let (tokens, missing) =
                        self.resolve_mentions(channel_id, &mention_names).await;
                    mention_tokens = tokens;
                    unresolved = missing;
                    if mention_tokens.is_empty() {
                        return ToolResult::error(format!(
                            "None of the mentioned employees resolved in this channel: {}. \
                             Check names with loop(resource: \"channel\", action: \"members\", \
                             channel_id: \"{}\"). The message was NOT sent.",
                            unresolved.join(", "),
                            channel_id
                        ));
                    }
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

                let content = if mention_tokens.is_empty() {
                    text.to_string()
                } else {
                    format!("{} {}", mention_tokens.join(" "), text)
                };
                // Agent-sent channel messages carry senderKind so receiving bots
                // apply handoff guardrails (depth cap, no engagement window).
                let mut metadata = HashMap::new();
                metadata.insert("senderKind".to_string(), "agent".to_string());
                if handoff_depth > 0 {
                    metadata.insert("handoffDepth".to_string(), handoff_depth.to_string());
                }

                let msg = comm::CommMessage {
                    id: uuid::Uuid::new_v4().to_string(),
                    from: String::new(),
                    to: String::new(),
                    topic: channel_id.to_string(),
                    conversation_id: channel_id.to_string(),
                    msg_type: comm::CommMessageType::LoopChannel,
                    content,
                    metadata,
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
                    Ok(()) => {
                        let mut note = if had_file {
                            format!("Sent to channel {} with the attached file.", channel_id)
                        } else {
                            format!("Message sent to channel {}", channel_id)
                        };
                        if !mention_tokens.is_empty() {
                            note.push_str(&format!(
                                " Handed off to {} mentioned employee(s).",
                                mention_tokens.len()
                            ));
                        }
                        if !unresolved.is_empty() {
                            note.push_str(&format!(
                                " Could not resolve: {} — sent without mentioning them.",
                                unresolved.join(", ")
                            ));
                        }
                        ToolResult::ok(note)
                    }
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

    async fn handle_loop(&self, input: &serde_json::Value) -> ToolResult {
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
                        "loop get",
                        "loop_id",
                        "loop(resource: \"loop\", action: \"get\", loop_id: \"...\")",
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
                        "loop members",
                        "loop_id",
                        "loop(resource: \"loop\", action: \"members\", loop_id: \"...\")",
                    ));
                }
                match self.comm.list_channel_members(loop_id).await {
                    Ok(members) => {
                        ToolResult::ok(serde_json::to_string_pretty(&members).unwrap_or_default())
                    }
                    Err(e) => ToolResult::error(format!("Failed to list loop members: {}. Do not retry — this is a communication error.", e)),
                }
            }
            _ => ToolResult::error(format!(
                "Unknown loop action: {}. Available: list, get, members",
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
            "status" => {
                let connected = self.comm.is_connected();
                let plugin_name = self.comm.name();
                let plugin_version = self.comm.version();

                ToolResult::ok(format!(
                    "Comm plugin: {} v{}\nConnected: {}",
                    plugin_name, plugin_version, connected
                ))
            }
            _ => ToolResult::error(format!(
                "Unknown topic action: {}. Available: subscribe, unsubscribe, status",
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
        "NeboAI communication — loops (workspaces this agent belongs to), channels, direct messages, and topics.\n\
         USE THIS when: user asks which loops you belong to, wants to message another bot, post to a channel, or interact with NeboAI infrastructure.\n\n\
         - loop(resource: \"loop\", action: \"list\") — List the loops this agent belongs to\n\
         - loop(resource: \"loop\", action: \"get\", loop_id: \"...\") / members — Loop details / members\n\
         - loop(resource: \"dm\", action: \"send\", to: \"agent-uuid\", text: \"Hello\") — Send a DM to another bot\n\
         - loop(resource: \"channel\", action: \"send\", channel_id: \"...\", text: \"Hello\") — Send to a loop channel\n\
         - loop(resource: \"channel\", action: \"send\", channel_id: \"...\", text: \"...\", mention: [\"Executive Assistant\"]) — Hand off to other AI employees: mentioned employees pick the message up and run\n\
         - loop(resource: \"channel\", action: \"share\", path: \"/abs/path/file.pdf\") — Share a local file into the channel reply\n\
         - loop(resource: \"dm\", action: \"share\", path: \"/abs/path/file.pdf\") — Share a local file in a direct message\n\
         - loop(resource: \"channel\", action: \"ensure\", name: \"daily-briefing\", description: \"...\") — Create (or get) a channel\n\
         - loop(resource: \"channel\", action: \"list\") — List available channels\n\
         - loop(resource: \"channel\", action: \"messages\", channel_id: \"...\", limit: 20) — Read channel messages\n\
         - loop(resource: \"channel\", action: \"members\", channel_id: \"...\") — List channel members\n\
         - loop(resource: \"topic\", action: \"subscribe\", topic: \"news\") / unsubscribe / status\n\n\
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
                    "enum": ["dm", "channel", "loop", "topic"]
                },
                "action": {
                    "type": "string",
                    "description": "The operation to perform on the selected resource. Never put a resource name here.",
                    "enum": ["send", "share", "ensure", "messages", "members", "list", "get", "subscribe", "unsubscribe", "status"]
                },
                "text": { "type": "string", "description": "Message text" },
                "path": { "type": "string", "description": "Absolute path of a local file to share (for channel/dm share)" },
                "to": { "type": "string", "description": "Recipient agent ID (for dm)" },
                "channel_id": { "type": "string", "description": "Channel ID" },
                "mention": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Employee names or @slugs to hand this channel message to. They are resolved to mention tokens and the mentioned employees respond (channel send only)."
                },
                "topic": { "type": "string", "description": "Topic name for pub/sub" },
                "loop_id": { "type": "string", "description": "Loop ID" },
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
        ctx: &'a ToolContext,
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
                    &["dm", "channel", "loop", "topic"],
                );
                if corrected.is_empty() {
                    self.infer_resource(&domain_input.action).to_string()
                } else {
                    corrected
                }
            };

            if resource.is_empty() {
                return ToolResult::error(
                    "Resource is required. Available: dm, channel, loop, topic",
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
                "dm" => self.handle_dm(&input, ctx.handoff_depth).await,
                "channel" => self.handle_channel(&input, ctx.handoff_depth).await,
                "loop" => self.handle_loop(&input).await,
                // Old name — return a correction, same pattern as the other
                // tool renames (the concept is user-facing "loop" everywhere).
                "group" => ToolResult::error(
                    "resource \"group\" is now \"loop\". Call \
                     loop(resource: \"loop\", action: \"list\") to list the loops \
                     this agent belongs to (or get / members with loop_id).",
                ),
                "topic" => self.handle_topic(&input).await,
                other => ToolResult::error(format!(
                    "Resource {:?} not available. Available: dm, channel, loop, topic",
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
