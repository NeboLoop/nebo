//! Emit tool — allows workflow activities and chat runs to fire events into
//! the EventBus.
//!
//! Always available to workflow activities (injected by the engine). No tool
//! declaration needed in the activity's `tools` array.

use std::sync::Arc;

use crate::events::{emit_source_for, Event, EventBus};
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// Who is speaking. An event's address names the seat that produced the fact,
/// so the emit tool must be able to answer this before it can build a source.
enum Producer {
    /// No owning seat (a standalone workflow run with no agent).
    None,
    /// The workflow paths: the run's agent is known when the tool is built.
    Seat(String),
    /// The chat registry: ONE shared tool serves every employee, so the seat
    /// is read from the run's session key at execution time.
    FromSession(Arc<db::Store>),
}

/// Tool that emits events into the EventBus.
pub struct EmitTool {
    bus: EventBus,
    producer: Producer,
}

impl EmitTool {
    pub fn new(bus: EventBus) -> Self {
        Self { bus, producer: Producer::None }
    }

    /// The emitting seat, known up front (the workflow executors).
    pub fn with_producer(mut self, producer: impl Into<String>) -> Self {
        let p = producer.into();
        self.producer = if p.is_empty() { Producer::None } else { Producer::Seat(p) };
        self
    }

    /// The emitting seat, read from the run's session key (the chat registry,
    /// where one tool instance serves every employee).
    pub fn with_session_producer(mut self, store: Arc<db::Store>) -> Self {
        self.producer = Producer::FromSession(store);
        self
    }

    /// The slug of the seat raising this event, or "" when no seat owns the run.
    fn producer_slug(&self, ctx: &ToolContext) -> String {
        match &self.producer {
            Producer::None => String::new(),
            Producer::Seat(slug) => slug.clone(),
            Producer::FromSession(store) => {
                let agent_id = types::keyparser::extract_agent_id(&ctx.session_key);
                if agent_id.is_empty() {
                    return String::new();
                }
                store
                    .get_agent(&agent_id)
                    .ok()
                    .flatten()
                    .map(|a| db::agent_slug(&a.name))
                    .unwrap_or(agent_id)
            }
        }
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


    fn search_hint(&self) -> &str {
        "emit event trigger automations"
    }

    /// The event goes on the local bus and nothing leaves the machine;
    /// whatever a subscribed workflow then does is judged in its own run.
    fn read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        false
    }

    fn rule_key(&self, _input: &serde_json::Value) -> String {
        "emit_event".to_string()
    }

    /// Pre-interface: it settles its own call shapes (see
    /// `DynTool::validates_input`).
    fn validates_input(&self) -> bool {
        false
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let named = match input["source"].as_str() {
                Some(s) if !s.trim().is_empty() => s.trim().to_string(),
                _ => return ToolResult::error(crate::errors::missing_param(
                    "emit",
                    "source",
                    "emit(source: \"my-workflow\", payload: {\"key\": \"value\"})",
                )),
            };

            // The ONE addressing function: a registered company event goes out
            // bare, a seat's own event is addressed by the seat exactly once.
            let producer = self.producer_slug(ctx);
            let source = emit_source_for(&producer, &named);

            let mut payload = input
                .get("payload")
                .cloned()
                .unwrap_or(serde_json::json!({}));
            if let (false, Some(obj)) = (producer.is_empty(), payload.as_object_mut()) {
                // The producing seat rides every payload, so a subscriber to an
                // un-namespaced company event knows who spoke.
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
