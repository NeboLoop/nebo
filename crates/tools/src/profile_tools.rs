//! The bot's own identity and account: `get_profile` reads the plan and
//! model, `update_profile` renames the employee or changes its role,
//! `open_billing` opens the billing portal.

use std::sync::Arc;

use db::Store;
use serde_json::{Value, json};

use crate::message_tool::NotifyFn;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

pub struct Profile {
    store: Arc<Store>,
    /// The registry's broadcast cell: a rename emits the same
    /// `agent_updated` event the updateAgent handler does.
    notify_fn: Arc<std::sync::RwLock<Option<NotifyFn>>>,
}

impl Profile {
    pub fn new(store: Arc<Store>, notify_fn: Arc<std::sync::RwLock<Option<NotifyFn>>>) -> Self {
        Self { store, notify_fn }
    }

    pub fn tools(self) -> Vec<Box<dyn DynTool>> {
        let profile = Arc::new(self);
        [ProfileOp::Get, ProfileOp::Update, ProfileOp::OpenBilling]
            .into_iter()
            .map(|op| {
                Box::new(ProfileTool {
                    op,
                    profile: profile.clone(),
                }) as Box<dyn DynTool>
            })
            .collect()
    }

    fn api(&self) -> Result<comm::api::NeboAIApi, String> {
        crate::build_neboai_api(&self.store)
    }

    async fn get(&self, ctx: &ToolContext) -> ToolResult {
        let api = match self.api() {
            Ok(a) => a,
            Err(e) => return ToolResult::error(e),
        };
        let mut out = format!("Bot ID: {}\n", api.bot_id());
        // Resolved "provider/model" of the invoking run (resolved ID only —
        // upstream model lineage is never exposed).
        if let Some(ref model) = ctx.model_preference {
            out.push_str(&format!("Model: {model}\n"));
        }
        match api.billing_subscription().await {
            Ok(v) => out.push_str(&summarize_subscription(&v)),
            Err(e) => out.push_str(&format!("Plan: unknown (billing lookup failed: {e})")),
        }
        ToolResult::ok(out)
    }

    async fn update(&self, input: &Value, ctx: &ToolContext) -> ToolResult {
        let api = match self.api() {
            Ok(a) => a,
            Err(e) => return ToolResult::error(e),
        };
        let name = input["name"].as_str().unwrap_or("");
        let role = input["role"].as_str().unwrap_or("");
        // Local identity FIRST: agent_profile.name is what feeds
        // {agent_name} in the system prompt and the UI. Without this
        // the rename was cosmetic — a memory said "Javis" while the
        // prompt still said "You are Nebo", and every new session
        // answered with the old name.
        if let Err(e) = self.store.update_agent_profile(
            (!name.is_empty()).then_some(name),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            (!role.is_empty()).then_some(role),
            None,
            None,
            None,
            None,
            None,
        ) {
            return ToolResult::error(format!(
                "Failed to update identity: {}. Do not retry — this is a database error.",
                e
            ));
        }
        // The agents-table row is what the UI reads (employee roster,
        // agent header, Identity settings) — agent_profile above only
        // feeds the prompt fallback. Without this the rename never
        // shows: agent_profile said 'Javis' while the roster still
        // said 'Nebo'. Reuses the same store.update_agent the
        // updateAgent handler uses (one canonical write path).
        let agent_id = {
            let id = types::keyparser::extract_agent_id(&ctx.session_key);
            if id.is_empty() {
                "assistant".to_string()
            } else {
                id
            }
        };
        let mut roster_row_missing = false;
        match self.store.get_agent(&agent_id) {
            Ok(Some(existing)) => {
                let new_name = if name.is_empty() {
                    existing.name.clone()
                } else {
                    name.to_string()
                };
                let new_desc = if role.is_empty() {
                    existing.description.clone()
                } else {
                    role.to_string()
                };
                if let Err(e) = self.store.update_agent(
                    &agent_id,
                    &new_name,
                    &new_desc,
                    &existing.agent_md,
                    &existing.frontmatter,
                    existing.pricing_model.as_deref(),
                    existing.pricing_cost,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                ) {
                    return ToolResult::error(format!(
                        "Failed to update agent record: {}. Do not retry — this is a database error.",
                        e
                    ));
                }
                // Live roster update — same event name + payload shape
                // the updateAgent handler broadcasts, so the sidebar
                // row and agent header patch in place immediately.
                let notify = self.notify_fn.read().ok().and_then(|g| g.clone());
                if let Some(notify) = notify {
                    notify(
                        "agent_updated",
                        serde_json::json!({
                            "agentId": agent_id,
                            "name": new_name,
                            "description": new_desc,
                        }),
                    );
                }
            }
            Ok(None) => {
                roster_row_missing = true;
                tracing::warn!(agent_id = %agent_id, "profile update: no agents row to sync");
            }
            Err(e) => {
                return ToolResult::error(format!(
                    "Failed to load agent record: {}. Do not retry — this is a database error.",
                    e
                ));
            }
        }
        // Cloud identity (NeboAI directory) — best-effort; the local
        // rename above is the one the user experiences.
        let mut caveats = String::new();
        if let Err(e) = api.update_bot_identity(name, role).await {
            tracing::warn!(error = %e, "cloud bot identity sync failed (local rename applied)");
            caveats.push_str(&format!(
                "; cloud directory sync failed ({}), local name is updated",
                e
            ));
        }
        if roster_row_missing {
            caveats.push_str("; no agent roster row found, only the prompt identity changed");
        }
        ToolResult::ok(format!(
            "Updated identity{}{}{}. This takes effect in new conversations.",
            if name.is_empty() {
                String::new()
            } else {
                format!(": name is now '{}'", name)
            },
            if role.is_empty() {
                String::new()
            } else {
                format!(", role: '{}'", role)
            },
            caveats,
        ))
    }

    async fn open_billing(&self) -> ToolResult {
        let api = match self.api() {
            Ok(a) => a,
            Err(e) => return ToolResult::error(e),
        };
        match api.billing_portal().await {
            Ok(v) => {
                let url = v.get("portalUrl").and_then(|u| u.as_str()).unwrap_or("");
                if url.is_empty() {
                    return ToolResult::error("Billing portal URL not available.");
                }
                open_url(url);
                ToolResult::ok(format!("Requested the system browser to open {url}."))
            }
            Err(e) => ToolResult::error(format!("Failed to open billing: {e}")),
        }
    }
}

/// Reduce the billing subscription document to the facts a bot needs: plan,
/// status, renewal, balance. Fields the service did not send are reported as
/// not reported, never invented, and the raw JSON stays out of the model's
/// context.
fn summarize_subscription(v: &Value) -> String {
    fn pick<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a Value> {
        keys.iter().find_map(|k| v.get(*k)).filter(|x| !x.is_null())
    }
    fn show(v: Option<&Value>) -> String {
        match v {
            None => "not reported".to_string(),
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
        }
    }
    let plan = pick(v, &["plan", "planName", "plan_name", "tier"]);
    let status = pick(v, &["status", "state"]);
    let renewal = pick(
        v,
        &[
            "renewsAt",
            "renews_at",
            "currentPeriodEnd",
            "current_period_end",
            "renewalDate",
            "renewal_date",
        ],
    );
    let balance = pick(
        v,
        &[
            "balance",
            "balanceCents",
            "balance_cents",
            "credits",
            "creditBalance",
            "credit_balance",
        ],
    );
    format!(
        "Plan: {}\nSubscription status: {}\nRenewal: {}\nBalance: {}",
        show(plan),
        show(status),
        show(renewal),
        show(balance)
    )
}

/// Open a URL in the system browser (best-effort, cross-platform).
fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let cmd = ("open", vec![url]);
    // Android (Termux) ships an `xdg-open` wrapper via termux-tools, so the
    // Linux command works there too; spawn stays best-effort either way.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let cmd = ("xdg-open", vec![url]);
    #[cfg(target_os = "windows")]
    let cmd = ("cmd", vec!["/C", "start", url]);
    let _ = std::process::Command::new(cmd.0).args(cmd.1).spawn();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProfileOp {
    Get,
    Update,
    OpenBilling,
}

struct ProfileTool {
    op: ProfileOp,
    profile: Arc<Profile>,
}

impl DynTool for ProfileTool {
    fn name(&self) -> &str {
        match self.op {
            ProfileOp::Get => "get_profile",
            ProfileOp::Update => "update_profile",
            ProfileOp::OpenBilling => "open_billing",
        }
    }

    fn description(&self) -> String {
        match self.op {
            ProfileOp::Get => "Reads this bot's account: its id, the model this conversation runs on, and the plan, subscription status, renewal and balance.".to_string(),
            ProfileOp::Update => "Renames you or changes your role, everywhere the owner sees it. When the owner says to use a different name, call this: remembering the name alone doesn't change it. Takes effect in new conversations.".to_string(),
            ProfileOp::OpenBilling => "Opens the account's billing portal in the owner's browser.".to_string(),
        }
    }

    fn schema(&self) -> Value {
        match self.op {
            ProfileOp::Update => json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "The new name." },
                    "role": { "type": "string", "description": "The new role, in a few words." }
                }
            }),
            ProfileOp::Get | ProfileOp::OpenBilling => {
                json!({ "type": "object", "properties": {} })
            }
        }
    }

    fn search_hint(&self) -> &str {
        match self.op {
            ProfileOp::Get => "account plan balance and model",
            ProfileOp::Update => "rename yourself or change role",
            ProfileOp::OpenBilling => "open the billing portal",
        }
    }

    fn read_only(&self, _input: &Value) -> bool {
        self.op == ProfileOp::Get
    }

    fn validate_input(&self, input: &Value) -> Result<(), String> {
        let blank = |k: &str| input[k].as_str().is_none_or(|s| s.trim().is_empty());
        if self.op == ProfileOp::Update && blank("name") && blank("role") {
            return Err("Give a name, a role, or both.".to_string());
        }
        Ok(())
    }

    fn activity(&self, _input: &Value) -> String {
        match self.op {
            ProfileOp::Get => "checking the account".to_string(),
            ProfileOp::Update => "updating my profile".to_string(),
            ProfileOp::OpenBilling => "opening billing".to_string(),
        }
    }

    fn outcome(&self, _input: &Value) -> String {
        match self.op {
            ProfileOp::Get => "Checked the account".to_string(),
            ProfileOp::Update => "Updated my profile".to_string(),
            ProfileOp::OpenBilling => "Opened billing".to_string(),
        }
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            match self.op {
                ProfileOp::Get => self.profile.get(ctx).await,
                ProfileOp::Update => self.profile.update(&input, ctx).await,
                ProfileOp::OpenBilling => self.profile.open_billing().await,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_subscription_reports_missing_fields_as_not_reported() {
        let v = json!({"plan": "solo", "status": "active", "secret_field": "x"});
        let out = summarize_subscription(&v);
        assert_eq!(
            out,
            "Plan: solo\nSubscription status: active\nRenewal: not reported\nBalance: not reported"
        );
        assert!(!out.contains("secret_field"));
    }

    #[test]
    fn an_update_needs_a_name_or_a_role() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(&dir.path().join("p.db").to_string_lossy()).unwrap());
        let tools = Profile::new(store, Arc::new(std::sync::RwLock::new(None))).tools();
        assert!(tools[1].validate_input(&json!({})).is_err());
        assert!(
            tools[1]
                .validate_input(&json!({"role": "Bookkeeper"}))
                .is_ok()
        );
    }
}
