//! `request_permission`: the way an employee turned away by a "never" rule
//! asks the owner for it back.
//!
//! A deny rule refuses a call in every mode, and the refusal names the
//! permission. When the owner says the employee may have it ("you can have
//! it"), the employee calls this with that permission. The call gives an
//! employee more room, so the permission check puts it on the owner's one
//! ask card (the Widens ask: the Inbox, the phone, the open chat, a voice
//! answer); it runs only as the owner's yes to this exact call, and then
//! lifts the rule: the deny goes, and a capability comes into the
//! employee's job. A rule a law or a package fixed stays.

use std::sync::Arc;

use serde_json::{Value, json};
use types::permissions::{CallEffects, Effect, Rule, RuleKey, RuleSource, Scope, Writer};

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

pub struct RequestPermissionTool {
    store: Arc<db::Store>,
}

impl RequestPermissionTool {
    pub const NAME: &'static str = "request_permission";

    pub fn new(store: Arc<db::Store>) -> Self {
        Self { store }
    }

    /// The deny rules that refuse `permission` for `agent_id`: its own and
    /// the company's, on the whole key (no narrower field).
    fn denies(&self, agent_id: &str, permission: &str) -> Vec<Rule> {
        self.store
            .permission_rules(agent_id)
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.effect == Effect::Deny && r.field.is_none() && r.key.value() == permission)
            .collect()
    }
}

fn permission(input: &Value) -> &str {
    input["permission"].as_str().unwrap_or("").trim()
}

/// The permission in the owner's words: a capability's label, else the key.
fn named(permission: &str) -> String {
    crate::capabilities::capability_label(permission).to_string()
}

impl DynTool for RequestPermissionTool {
    fn name(&self) -> &str {
        Self::NAME
    }

    fn description(&self) -> String {
        "Ask the owner to turn a permission back on for you, when a call was refused because it is turned off \
         (the refusal names the permission). Only when the owner wants you to have it: say what you needed it \
         for and offer this; if they agree, call it. It puts the question on the owner's card, and nothing \
         changes until they approve it there. Don't call it for anything the refusal didn't name."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "permission": { "type": "string", "description": "The permission the refusal named, e.g. shell." }
            },
            "required": ["permission"]
        })
    }

    fn search_hint(&self) -> &str {
        "turn permission back on owner approves"
    }

    /// It gives an employee more room: the check asks the owner, every time.
    fn effects(&self, _input: &Value) -> CallEffects {
        CallEffects { widens: true, ..CallEffects::unknown() }
    }

    fn activity(&self, input: &Value) -> String {
        let p = permission(input);
        format!("have \u{201c}{}\u{201d} turned back on", named(p))
    }

    fn outcome(&self, input: &Value) -> String {
        format!("\u{201c}{}\u{201d} is on again", named(permission(input)))
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let p = permission(&input);
            if p.is_empty() {
                return ToolResult::error("`permission` is required: the permission the refusal named, e.g. shell.");
            }
            // Only the owner gives an employee more room: this runs only as
            // the owner's answer to this exact call.
            let Some(ask_id) = ctx.answered_ask.clone() else {
                return ToolResult::error(
                    "Only the owner can turn a permission back on. Calling this puts the question on their card.",
                );
            };
            let agent_id = ctx.grant.as_ref().map(|g| g.agent_id.clone()).unwrap_or_default();
            let denies = self.denies(&agent_id, p);
            if denies.is_empty() {
                return ToolResult::ok(format!(
                    "Nothing turns \u{201c}{}\u{201d} off for you now. Try the call again.",
                    named(p)
                ));
            }
            if denies.iter().any(|r| r.locked) {
                return ToolResult::error(format!(
                    "\u{201c}{}\u{201d} is fixed by a law or a package, so it can't be turned back on here. Tell the owner.",
                    named(p)
                ));
            }
            let key: RuleKey = denies[0].key.clone();
            let company_wide = denies.iter().any(|r| r.scope == Scope::Company);
            for rule in &denies {
                if let Err(e) = self.store.remove_permission_rule(&rule.id, &Writer::Owner) {
                    return ToolResult::error(format!("The permission could not be turned back on: {e}"));
                }
            }
            // A capability comes into the employee's job, so the mode no
            // longer asks for it as work outside the job.
            if matches!(key, RuleKey::Capability(_)) && !agent_id.is_empty() {
                let rule = Rule {
                    id: uuid::Uuid::new_v4().to_string(),
                    scope: Scope::Employee(agent_id.clone()),
                    key,
                    field: None,
                    effect: Effect::Allow,
                    money: None,
                    source: RuleSource::AllowAlways { ask_id },
                    locked: false,
                    created_at: chrono::Utc::now().timestamp(),
                };
                if let Err(e) = self.store.write_permission_rule(&rule, &Writer::Owner) {
                    return ToolResult::error(format!("The permission could not be turned back on: {e}"));
                }
            }
            // A company "never" bound every employee; lifting it is the
            // owner's company setting, and he hears so.
            let others = if company_wide {
                " It was off for every employee; the others now follow their own settings for it, so tell the owner that too."
            } else {
                ""
            };
            ToolResult::ok(format!(
                "The owner turned \u{201c}{}\u{201d} back on for you. Go ahead with what you needed it for.{others}",
                named(p)
            ))
        })
    }
}
