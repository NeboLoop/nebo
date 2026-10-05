//! The bot's own identity and account: `get_profile` reads the plan and
//! model, `update_profile` renames the employee or changes its role (its own
//! name only, never the bot's), `open_billing` opens the billing portal.

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
    /// The employee loader and the live roster: a rename is the ONE rename
    /// (`agent_tool::rename_employee`), which moves the employee's folder.
    agent_loader: Arc<napp::AgentLoader>,
    live: crate::agent_tool::AgentRegistry,
}

impl Profile {
    pub fn new(
        store: Arc<Store>,
        notify_fn: Arc<std::sync::RwLock<Option<NotifyFn>>>,
        agent_loader: Arc<napp::AgentLoader>,
        live: crate::agent_tool::AgentRegistry,
    ) -> Self {
        Self { store, notify_fn, agent_loader, live }
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

    /// Rename the employee in this conversation, or change its role. Only its
    /// own row changes: the bot's name belongs to the owner (Bot settings, the
    /// web, the phone), and the old single agent profile is not a name any
    /// employee goes by.
    async fn update(&self, input: &Value, ctx: &ToolContext) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("").trim();
        let role = input["role"].as_str().unwrap_or("").trim();
        let agent_id = {
            let id = types::keyparser::extract_agent_id(&ctx.session_key);
            if id.is_empty() {
                crate::team_tool::PRIMARY_AGENT_ID.to_string()
            } else {
                id
            }
        };
        let existing = match self.store.get_agent(&agent_id) {
            Ok(Some(existing)) => existing,
            Ok(None) => {
                return ToolResult::error(format!(
                    "There is no employee '{agent_id}' here to rename."
                ));
            }
            Err(e) => {
                return ToolResult::error(format!(
                    "Failed to load agent record: {}. Do not retry — this is a database error.",
                    e
                ));
            }
        };
        let new_desc = if role.is_empty() {
            existing.description.clone()
        } else {
            role.to_string()
        };
        // The name through the ONE rename: the same employee, its folder
        // moved with it.
        let folder = match crate::agent_tool::rename_employee(&self.store, &self.agent_loader, &self.live, &agent_id, name).await {
            Ok(folder) => folder,
            Err(e) => return ToolResult::error(format!("Not renamed: {e}")),
        };
        let new_name = if name.is_empty() { existing.name.clone() } else { name.to_string() };
        // The same store.update_agent the updateAgent handler uses (one
        // canonical write path) for the role.
        if !role.is_empty()
            && let Err(e) = self.store.update_agent(
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
            )
        {
            return ToolResult::error(format!(
                "Failed to update agent record: {}. Do not retry — this is a database error.",
                e
            ));
        }
        // Live roster update — same event name + payload shape the
        // updateAgent handler broadcasts, so the sidebar row and agent header
        // patch in place immediately.
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
        ToolResult::ok(format!(
            "Updated identity{}{}. This takes effect in new conversations.",
            if name.is_empty() {
                String::new()
            } else if folder.is_empty() {
                format!(": name is now '{}', and everything else is kept", name)
            } else {
                format!(": name is now '{}', and everything else is kept ({folder})", name)
            },
            if role.is_empty() {
                String::new()
            } else {
                format!(", role: '{}'", role)
            },
        ))
    }

    /// The billing portal, opened in the owner's browser. Asked from the
    /// phone app, no portal is fetched and no link is given: billing is in
    /// the app (`crate::store_app`).
    async fn open_billing(&self, ctx: &ToolContext) -> ToolResult {
        if ctx.in_store_app() {
            return ToolResult::ok(format!(
                "Billing is in the app on the owner's phone. Tell them: {}",
                crate::store_app::PLAN_IN_APP
            ));
        }
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
    let _ = command::new::<std::process::Command>(cmd.0, command::Console::Hidden).args(cmd.1).spawn();
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
            ProfileOp::Update => "Renames you or changes your role, everywhere the owner sees it. A rename keeps everything: you stay the same employee, with your id, settings, schedules, memory, chats and files (an app keeps its page, source and data). Never delete yourself to rename. It changes your own name only, never the bot's. When the owner says to use a different name, call this: remembering the name alone doesn't change it. Takes effect in new conversations.".to_string(),
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
                ProfileOp::OpenBilling => self.profile.open_billing(ctx).await,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loader(dir: &tempfile::TempDir) -> Arc<napp::AgentLoader> {
        Arc::new(napp::AgentLoader::new(dir.path().join("installed"), dir.path().join("agents")))
    }

    fn live() -> crate::agent_tool::AgentRegistry {
        Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()))
    }

    /// From the phone app, billing is an instruction, never a link: no
    /// portal is fetched (the store has no NeboAI credentials, so a fetch
    /// would fail) and nothing is opened.
    #[tokio::test]
    async fn billing_from_the_phone_app_is_an_instruction_with_no_link() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(&dir.path().join("p.db").to_string_lossy()).unwrap());
        let profile = Profile::new(store, Arc::new(std::sync::RwLock::new(None)), loader(&dir), live());
        for phone in ["ios", "android"] {
            let ctx = ToolContext { platform: Some(phone.into()), ..ToolContext::new(crate::Origin::User) };
            let r = profile.open_billing(&ctx).await;
            assert!(!r.is_error, "{phone}: {}", r.content);
            assert!(r.content.contains("Open Settings → Account → Plan in the app."), "{phone}: {}", r.content);
            assert!(!r.content.contains("http"), "{phone}: {}", r.content);
        }
        // Not from the phone: the portal is fetched as before (and fails
        // here, with no NeboAI account).
        let r = profile.open_billing(&ToolContext::new(crate::Origin::User)).await;
        assert!(r.is_error, "{}", r.content);
    }

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

    /// An employee renaming itself in chat changes its own name and nothing
    /// else: not the primary, not the old agent profile, and nothing reaches
    /// NeboAI — the store here has no NeboAI credentials at all, so any call
    /// to the hub would fail the rename.
    #[tokio::test]
    async fn a_self_rename_changes_only_the_employees_own_name() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(&dir.path().join("p.db").to_string_lossy()).unwrap());
        store.ensure_agent_profile().unwrap();
        for (id, name) in [(crate::team_tool::PRIMARY_AGENT_ID, "Nanna"), ("books", "Bookkeeper")] {
            store.create_agent(id, None, name, "", "", "{}", None, None).unwrap();
        }
        let tools = Profile::new(store.clone(), Arc::new(std::sync::RwLock::new(None)), loader(&dir), live()).tools();
        let ctx = ToolContext { session_key: "agent:books:web".into(), ..Default::default() };

        let out = tools[1].execute_dyn(&ctx, json!({"name": "Penny", "role": "Books"})).await;
        assert!(!out.is_error, "{}", out.content);

        assert_eq!(store.get_agent("books").unwrap().unwrap().name, "Penny");
        assert_eq!(store.get_agent("books").unwrap().unwrap().description, "Books");
        assert_eq!(store.get_agent(crate::team_tool::PRIMARY_AGENT_ID).unwrap().unwrap().name, "Nanna");
        let profile = store.get_agent_profile().unwrap().unwrap();
        assert_eq!(profile.name, "Nebo", "the old agent profile is not renamed");
        assert!(profile.role.unwrap_or_default().is_empty(), "no bot-wide role is written");

        // The primary renaming itself is the same: its own row only.
        let ctx = ToolContext { session_key: "agent:assistant:web".into(), ..Default::default() };
        let out = tools[1].execute_dyn(&ctx, json!({"name": "Ada"})).await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(store.get_agent(crate::team_tool::PRIMARY_AGENT_ID).unwrap().unwrap().name, "Ada");
        assert_eq!(store.get_agent_profile().unwrap().unwrap().name, "Nebo");
    }

    /// "Rename yourself": an app renaming itself is the ONE rename, so it
    /// stays the same employee and its folder moves with the name; a name
    /// another employee has is refused.
    #[tokio::test]
    async fn an_app_renaming_itself_keeps_everything() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(&dir.path().join("p.db").to_string_lossy()).unwrap());
        store.create_agent("tweet", Some("user"), "Tweet", "A game.", "", "{}", None, None).unwrap();
        store.create_agent("other", None, "Bookkeeper", "", "", "{}", None, None).unwrap();
        let folder = dir.path().join("agents").join("Tweet");
        std::fs::create_dir_all(folder.join("src")).unwrap();
        std::fs::write(folder.join("src").join("App.tsx"), "game").unwrap();
        store.set_agent_napp_path("tweet", &folder.to_string_lossy()).unwrap();
        let tools = Profile::new(store.clone(), Arc::new(std::sync::RwLock::new(None)), loader(&dir), live()).tools();
        let ctx = ToolContext { session_key: "agent:tweet:web".into(), ..Default::default() };

        let out = tools[1].execute_dyn(&ctx, json!({"name": "bookkeeper"})).await;
        assert!(out.is_error && out.content.contains("already have an employee named"), "{}", out.content);
        assert_eq!(store.get_agent("tweet").unwrap().unwrap().name, "Tweet");

        let out = tools[1].execute_dyn(&ctx, json!({"name": "Flip-Flap"})).await;
        assert!(!out.is_error, "{}", out.content);
        let row = store.get_agent("tweet").unwrap().unwrap();
        assert_eq!((row.name.as_str(), row.description.as_str()), ("Flip-Flap", "A game."));
        let moved = dir.path().join("agents").join("Flip-Flap");
        assert_eq!(row.napp_path.as_deref(), Some(moved.to_str().unwrap()));
        assert_eq!(std::fs::read_to_string(moved.join("src").join("App.tsx")).unwrap(), "game");
        assert!(!folder.exists());
    }

    #[test]
    fn an_update_needs_a_name_or_a_role() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(&dir.path().join("p.db").to_string_lossy()).unwrap());
        let tools = Profile::new(store, Arc::new(std::sync::RwLock::new(None)), loader(&dir), live()).tools();
        assert!(tools[1].validate_input(&json!({})).is_err());
        assert!(
            tools[1]
                .validate_input(&json!({"role": "Bookkeeper"}))
                .is_ok()
        );
    }
}
