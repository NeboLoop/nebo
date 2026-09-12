//! Emit tool — allows workflow activities to fire events into the EventBus.
//!
//! Always available to workflow activities (injected by the engine). No tool
//! declaration needed in the activity's `tools` array.

use crate::events::{Event, EventBus};
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// Tool that emits events into the EventBus.
pub struct EmitTool {
    bus: EventBus,
    /// The emitting seat's slug, stamped on every payload as `producer` so
    /// a subscriber to an un-namespaced company event knows who spoke.
    producer: Option<String>,
}

impl EmitTool {
    pub fn new(bus: EventBus) -> Self {
        Self { bus, producer: None }
    }

    pub fn with_producer(mut self, producer: impl Into<String>) -> Self {
        let p = producer.into();
        self.producer = (!p.is_empty()).then_some(p);
        self
    }
}

impl DynTool for EmitTool {
    fn name(&self) -> &str {
        "emit"
    }

    fn description(&self) -> String {
        "Emit an event that can trigger other workflows. Provide a source string (e.g. 'email.urgent') and optional payload object.".to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "source": {
                    "type": "string",
                    "description": "Event source identifier, e.g. 'email.urgent' or 'lead.qualified'"
                },
                "payload": {
                    "type": "object",
                    "description": "Optional event payload data"
                }
            },
            "required": ["source"]
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
            let source = match input["source"].as_str() {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => return ToolResult::error(crate::errors::missing_param(
                    "emit",
                    "source",
                    "emit(source: \"my-workflow\", payload: {\"key\": \"value\"})",
                )),
            };

            let mut payload = input
                .get("payload")
                .cloned()
                .unwrap_or(serde_json::json!({}));
            if let (Some(producer), Some(obj)) = (&self.producer, payload.as_object_mut()) {
                obj.entry("producer").or_insert_with(|| serde_json::json!(producer));
            }

            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            self.bus.emit(Event {
                source: source.clone(),
                payload,
                origin: ctx.session_key.clone(),
                timestamp,
            });

            ToolResult::ok(format!("Event emitted: {}", source))
        })
    }
}
