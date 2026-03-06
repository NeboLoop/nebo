use std::sync::Arc;

use db::Store;
use crate::domain::DomainInput;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// MessageTool handles outbound delivery to the owner (notifications, companion chat).
pub struct MessageTool {
    store: Arc<Store>,
}

impl MessageTool {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    fn infer_resource(&self, action: &str) -> &str {
        match action {
            "notify" => "owner",
            "alert" | "speak" | "dnd_status" => "notify",
            "conversations" | "read" => "sms",
            _ => "",
        }
    }
}

impl DynTool for MessageTool {
    fn name(&self) -> &str {
        "message"
    }

    fn description(&self) -> String {
        "Send messages and notifications to the owner.\n\n\
         Resources and Actions:\n\
         - owner: notify (append message to companion chat + push notification)\n\
         - notify: send, alert (system notifications)\n\n\
         Examples:\n  \
         message(resource: \"owner\", action: \"notify\", text: \"Your task is complete!\")\n  \
         message(action: \"notify\", text: \"Reminder: meeting in 5 minutes\")"
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "resource": {
                    "type": "string",
                    "description": "Resource type: owner, notify, sms",
                    "enum": ["owner", "notify", "sms"]
                },
                "action": {
                    "type": "string",
                    "description": "Action to perform",
                    "enum": ["notify", "send", "alert", "speak"]
                },
                "text": { "type": "string", "description": "Message text" },
                "title": { "type": "string", "description": "Notification title" }
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

            match resource.as_str() {
                "owner" => {
                    let text = input["text"].as_str().unwrap_or("");
                    if text.is_empty() {
                        return ToolResult::error("text is required");
                    }

                    // Get existing companion chat or create one
                    let msg_id = uuid::Uuid::new_v4().to_string();
                    let companion = match self.store.get_companion_chat_by_user("") {
                        Ok(Some(chat)) => Ok(chat),
                        _ => {
                            let chat_id = uuid::Uuid::new_v4().to_string();
                            self.store.get_or_create_companion_chat(&chat_id, "")
                        }
                    };

                    match companion {
                        Ok(chat) => {
                            let _ = self.store.create_chat_message(
                                &msg_id,
                                &chat.id,
                                "assistant",
                                text,
                                None,
                            );
                            // Fire OS notification
                            notify_crate::send("Nebo", text);
                            ToolResult::ok(format!("Notified owner: {}", text))
                        }
                        Err(e) => ToolResult::error(format!("Failed to notify: {}", e)),
                    }
                }
                "notify" => {
                    let text = input["text"].as_str().unwrap_or("");
                    let title = input["title"].as_str().unwrap_or("Nebo");

                    if text.is_empty() {
                        return ToolResult::error("text is required");
                    }

                    let id = uuid::Uuid::new_v4().to_string();
                    match self.store.create_notification(
                        &id,
                        "",  // user_id (single-user app)
                        "info",
                        title,
                        Some(text),
                        None,
                        None,
                    ) {
                        Ok(_) => {
                            // Fire OS notification
                            notify_crate::send(title, text);
                            ToolResult::ok(format!("Notification sent: {}", text))
                        }
                        Err(e) => ToolResult::error(format!("Failed to send notification: {}", e)),
                    }
                }
                "sms" => ToolResult::error(
                    "SMS messaging is not available on this platform.",
                ),
                other => ToolResult::error(format!(
                    "Resource {:?} not available. Available: owner, notify",
                    other
                )),
            }
        })
    }
}
