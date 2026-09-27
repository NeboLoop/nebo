//! The bot's own hosted mail address as a provider of `mail.message.send`.
//!
//! A bot paired with a hosted service can have an address of its own
//! (`nanna-7kq@nebo.bot`). Sending from it is not a second mail tool: it is
//! one more provider of the one operation, `mail_message_send`, next to any
//! connected mail plugin (the business's own mailbox). The provider exists
//! only while the address does — the server registers it after the hub
//! confirms the address and removes it when the pairing goes — so a Nebo
//! with no hosted service has exactly the tools it had before.
//!
//! Who is writing is never the model's to say: the employee whose run makes
//! the call signs it (its `+tag` brings replies back to it) and the
//! conversation it is made from rides along, so an answer comes back there.

use std::sync::Arc;

use serde_json::{json, Map, Value};

use crate::effects::SendOutcome;
use crate::operation_tools::{OperationProvider, ProvidedOperation};
use crate::origin::ToolContext;
use crate::registry::ToolResult;
use db::Store;

/// The provider's name, the `provider` value when several bind the send.
pub const PROVIDER: &str = "bot-address";

/// The one operation it performs.
const OPERATION: &str = "mail.message.send";

/// The bot's own address, sending through the hub that hosts it.
pub struct BotMailProvider {
    store: Arc<Store>,
    api_url: String,
    address: String,
}

impl BotMailProvider {
    pub fn new(store: Arc<Store>, api_url: String, address: String) -> Self {
        Self { store, api_url, address }
    }

    /// The employee making the call: its tag and name, or none for the
    /// primary (the bot's own name signs its mail).
    fn signer(&self, ctx: &ToolContext) -> (String, String) {
        let agent_id = types::keyparser::extract_agent_id(&ctx.session_key);
        if agent_id.is_empty() || agent_id == crate::team_tool::PRIMARY_AGENT_ID {
            return (String::new(), String::new());
        }
        match self.store.get_agent(&agent_id) {
            Ok(Some(a)) if !a.name.trim().is_empty() => (comm::handle::slugify(&a.name), a.name.trim().to_string()),
            _ => (String::new(), String::new()),
        }
    }

    /// The request the hub is sent for this call.
    pub fn request(&self, ctx: &ToolContext, input: &Value) -> Result<comm::api_types::BotEmailSend, String> {
        let text = |k: &str| input.get(k).and_then(|v| v.as_str()).map(str::trim).unwrap_or("").to_string();
        let to_owner = input.get("toOwner").and_then(|v| v.as_bool()).unwrap_or(false);
        let to = match input.get("to") {
            Some(Value::Array(items)) => items.iter().filter_map(|v| v.as_str()).map(str::trim).collect::<Vec<_>>().join(","),
            _ => text("to"),
        };
        if !to_owner && to.is_empty() {
            return Err("Say who it goes to: `to` (an address), or `toOwner: true` to write to the owner.".into());
        }
        if !to_owner && to.contains(',') {
            return Err("One recipient per message from the bot's own address: send one call per person.".into());
        }
        let body = text("text");
        if body.is_empty() {
            return Err("`text` is the message and it is empty; write the message in `text`.".into());
        }
        let (employee, employee_name) = self.signer(ctx);
        let agent_id = types::keyparser::extract_agent_id(&ctx.session_key);
        let chat_id = if ctx.session_id.is_empty() {
            String::new()
        } else {
            self.store.resolve_session_chat_id(&ctx.session_id)
        };
        Ok(comm::api_types::BotEmailSend {
            to: if to_owner { String::new() } else { to },
            to_owner,
            subject: text("subject"),
            body_text: body,
            body_html: text("html"),
            employee,
            employee_name,
            inbound_email_id: String::new(),
            agent_id,
            chat_id,
        })
    }

    async fn send(&self, req: comm::api_types::BotEmailSend) -> SendOutcome {
        let Some(bot_id) = config::read_bot_id() else {
            return SendOutcome::PreSendFailure("This Nebo has no bot id; the hosted address cannot send.".into());
        };
        let Some(token) = auth::neboai_token(&self.store) else {
            return SendOutcome::PreSendFailure("This Nebo is no longer paired; the hosted address cannot send.".into());
        };
        let api = comm::api::NeboAIApi::new(self.api_url.clone(), bot_id, token);
        let to = if req.to_owner { "the owner".to_string() } else { req.to.clone() };
        match api.send_bot_email(&req).await {
            Ok(v) => {
                let reference = v["messageId"].as_str().map(str::to_string);
                SendOutcome::Sent(format!("Sent from {} to {to}.", self.address), reference)
            }
            Err(e @ comm::CommError::Paused) => SendOutcome::PreSendFailure(e.to_string()),
            Err(comm::CommError::Http { status, body }) if (400..500).contains(&status) => {
                let why = serde_json::from_str::<Value>(&body)
                    .ok()
                    .and_then(|v| v["error"].as_str().map(str::to_string))
                    .unwrap_or(body);
                SendOutcome::ConfirmedFailure(format!("Not sent from {}: {why}", self.address))
            }
            // A server error or no answer: the mail may already be on its way.
            Err(e) => SendOutcome::Unknown(format!("sending from {}: {e}", self.address)),
        }
    }
}

impl OperationProvider for BotMailProvider {
    fn provider(&self) -> &str {
        PROVIDER
    }

    fn service(&self) -> String {
        format!("your own address {}", self.address)
    }

    fn operations(&self) -> Vec<ProvidedOperation> {
        let mut properties = Map::new();
        properties.insert("to".into(), json!({"type": "string", "description": "The recipient's address."}));
        properties.insert(
            "toOwner".into(),
            json!({"type": "boolean", "description": "Write to the owner (their account email); `to` is not needed."}),
        );
        properties.insert("subject".into(), json!({"type": "string", "description": "The subject line."}));
        properties.insert("text".into(), json!({"type": "string", "description": "The message, plain text."}));
        properties.insert("html".into(), json!({"type": "string", "description": "Optional HTML version of the message."}));
        vec![ProvidedOperation {
            operation: OPERATION.to_string(),
            properties,
            required: vec!["subject".into(), "text".into()],
            note: format!(
                "{PROVIDER} sends from {}, signed by you; replies come back to you. Mail to the owner is never limited; \
                 mail to anyone else counts against the bot's daily limit.",
                self.address
            ),
            ..Default::default()
        }]
    }

    fn perform<'a>(
        &'a self,
        ctx: &'a ToolContext,
        operation: &'a str,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            if operation != OPERATION {
                return ToolResult::error(format!("{PROVIDER} performs {OPERATION} only."));
            }
            let req = match self.request(ctx, &input) {
                Ok(r) => r,
                Err(why) => return ToolResult::error(why),
            };
            crate::effects::guarded_send(&self.store, ctx, "messaging", PROVIDER, OPERATION, &input, || self.send(req)).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Arc<Store> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.db");
        std::mem::forget(dir);
        Arc::new(Store::new(&path.to_string_lossy()).unwrap())
    }

    fn provider() -> BotMailProvider {
        BotMailProvider::new(store(), "http://127.0.0.1:9".into(), "nanna-7kq@nebo.bot".into())
    }

    /// Mail to the owner names no address: the hub fills the owner's.
    #[test]
    fn mail_to_the_owner_asks_the_hub_to_address_it() {
        let ctx = ToolContext::new(crate::Origin::User).with_session("agent:assistant:web", "");
        let req = provider()
            .request(&ctx, &json!({"toOwner": true, "subject": "Done", "text": "The report is ready."}))
            .unwrap();
        assert!(req.to_owner);
        assert!(req.to.is_empty());
        assert!(req.employee.is_empty(), "the primary signs with the bot's own name, no tag");
        let wire = serde_json::to_value(&req).unwrap();
        assert_eq!(wire["toOwner"], true);
        assert!(wire.get("to").is_none());
        assert_eq!(wire["bodyText"], "The report is ready.");
    }

    /// An employee's mail carries its tag and name, taken from the run —
    /// never from the call.
    #[test]
    fn an_employees_mail_is_signed_with_its_tag_and_name() {
        let p = provider();
        p.store
            .create_agent("recep-1", None, "Front Desk", "", "", "---\nname: Front Desk\n---\n", None, None)
            .unwrap();
        let ctx = ToolContext::new(crate::Origin::User).with_session("agent:recep-1:web", "");
        let req = p
            .request(&ctx, &json!({"to": "pat@example.com", "subject": "Hi", "text": "Hello", "employee": "someone-else"}))
            .unwrap();
        assert_eq!(req.employee, "front-desk");
        assert_eq!(req.employee_name, "Front Desk");
        assert_eq!(req.agent_id, "recep-1");
        assert_eq!(req.to, "pat@example.com");
    }

    /// A send with nobody to send to, or nothing to say, is refused before
    /// anything leaves.
    #[test]
    fn a_send_needs_a_recipient_and_a_message() {
        let ctx = ToolContext::new(crate::Origin::User);
        let p = provider();
        assert!(p.request(&ctx, &json!({"subject": "x", "text": "y"})).is_err());
        assert!(p.request(&ctx, &json!({"to": "a@example.com", "subject": "x", "text": ""})).is_err());
        assert!(p.request(&ctx, &json!({"to": ["a@example.com", "b@example.com"], "subject": "x", "text": "y"})).is_err());
    }
}
