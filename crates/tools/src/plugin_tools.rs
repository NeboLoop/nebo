//! The model-facing plugin tools: one `plugin__<slug>` tool per installed
//! plugin (its command-line surface), `find_plugins` (the marketplace and its
//! install card) and `read_plugin_events`. The installed roster is the
//! deferred-tool listing itself. The work is the plugin runner's
//! (`plugin_tool::PluginRunner`); these are its interface.

use std::sync::Arc;

use crate::origin::ToolContext;
use crate::plugin_tool::{PluginCall, PluginRunner};
use crate::registry::{DynTool, ToolResult};

/// The namespace of an installed plugin's tool.
pub const PLUGIN_PREFIX: &str = "plugin__";
pub const FIND_PLUGINS: &str = "find_plugins";
pub const READ_PLUGIN_EVENTS: &str = "read_plugin_events";

/// Skill names a plugin's description lists before it says how many more.
const SKILLS_NAMED: usize = 12;

/// `plugin__<slug>`: the tool an installed plugin is called through.
pub fn plugin_tool_name(slug: &str) -> String {
    format!("{PLUGIN_PREFIX}{slug}")
}

/// The slug a `plugin__<slug>` tool name runs, or `None` for any other name.
pub fn plugin_slug(tool_name: &str) -> Option<&str> {
    tool_name.strip_prefix(PLUGIN_PREFIX).filter(|s| !s.is_empty())
}

/// An installed plugin's command-line surface, one tool per plugin (the
/// MCP-server analog).
pub struct PluginCliTool {
    name: String,
    slug: String,
    service: String,
    description: String,
    hint: String,
    runner: Arc<PluginRunner>,
}

impl PluginCliTool {
    pub fn new(runner: Arc<PluginRunner>, slug: &str) -> Self {
        let manifest = runner.plugin_store().get_manifest(slug);
        let service = manifest
            .as_ref()
            .map(|m| m.name.trim().to_string())
            .filter(|n| !n.is_empty() && n != slug)
            .unwrap_or_else(|| crate::humanize::service_name(slug));
        let description = Self::describe(&runner, slug, &service, manifest.as_ref());
        let hint = search_hint(&service, manifest.as_ref());
        Self { name: plugin_tool_name(slug), slug: slug.to_string(), service, description, hint, runner }
    }

    fn describe(
        runner: &PluginRunner,
        slug: &str,
        service: &str,
        manifest: Option<&napp::plugin::PluginManifest>,
    ) -> String {
        let blurb = manifest.map(|m| m.description.trim()).filter(|d| !d.is_empty());
        let mut out = match blurb {
            Some(b) => format!("Runs {service} commands. {b}\n"),
            None => format!("Runs {service} commands.\n"),
        };
        let skills: Vec<String> = runner.list_services(slug).into_iter().map(|(name, _)| name).collect();
        if skills.is_empty() {
            out.push_str("- `command` is the subcommand and flags; `help` lists them.\n");
        } else {
            let named = skills.iter().take(SKILLS_NAMED).cloned().collect::<Vec<_>>().join(", ");
            let more = skills.len().saturating_sub(SKILLS_NAMED);
            let more = if more > 0 { format!(" (and {more} more)") } else { String::new() };
            out.push_str(&format!(
                "- `command` is the subcommand and flags, as its skills document them. Load the \
                 skill with use_skill before the first command; don't guess flags: {named}{more}.\n"
            ));
        }
        out.push_str(
            "- Put values with quotes or special characters in `args` ({\"flag\": \"value\"}), not in `command`.\n\
             - It runs directly, with no shell: no pipes, redirects or `&&`.\n",
        );
        if let Some(auth) = manifest.and_then(|m| m.auth.as_ref()) {
            let label = if auth.label.is_empty() { service } else { auth.label.as_str() };
            if auth.profile_dir_env.is_some() {
                out.push_str(&format!(
                    "- Each employee uses its own {label} account; with none connected, a connect card appears on first use.\n"
                ));
            } else if !runner.plugin_store().is_ready(slug) {
                out.push_str(&format!(
                    "- Not connected yet: the owner connects {label} in Settings, Plugins. You can't sign in for them.\n"
                ));
            }
        }
        if runner.plugin_store().get_channel_def(slug).is_some() {
            out.push_str(
                "- Channel commands: `upload --channel <id> --path <absolute path>` shares a file, \
                 `post --channel <id> --text <text>` posts, `dm --user <id> --text <text>` messages \
                 one person. Replies to an incoming message need no command.\n",
            );
        }
        let gated: Vec<String> = manifest
            .map(|m| {
                let mut ops: Vec<String> = m
                    .interface_bindings
                    .keys()
                    .filter(|op| crate::interface_catalog::is_gated(op))
                    .map(|op| crate::operation_tools::operation_tool_name(op))
                    .collect();
                ops.sort();
                ops
            })
            .unwrap_or_default();
        if !gated.is_empty() {
            out.push_str(&format!(
                "- These run only through their own tools, never here: {}.\n",
                gated.join(", ")
            ));
        }
        out.trim_end().to_string()
    }
}

impl DynTool for PluginCliTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Subcommand and flags, as the plugin's skills document them. Leave out the plugin's own name."
                },
                "args": {
                    "type": "object",
                    "description": "Flags passed as separate arguments, each key as --key. Use it for values with quotes or special characters.",
                    "additionalProperties": { "type": "string" }
                },
                "timeout": {
                    "type": "integer",
                    "description": "Seconds the command may take (default 120)."
                },
                "display": {
                    "type": "string",
                    "description": "One plain sentence the owner reads if this command needs their approval, with real names and amounts."
                }
            },
            "required": ["command"],
            "additionalProperties": false
        })
    }

    fn search_hint(&self) -> &str {
        &self.hint
    }

    fn rule_key(&self, _input: &serde_json::Value) -> String {
        self.name.clone()
    }

    /// `args: {command: "doctor"}` is the command, not a `--command` flag:
    /// the model nests the one field it was asked for under the object it
    /// was also offered (live, 2026-09-06).
    fn normalize_input(&self, mut input: serde_json::Value) -> serde_json::Value {
        let has_command = input
            .get("command")
            .and_then(|c| c.as_str())
            .is_some_and(|c| !c.trim().is_empty());
        if has_command {
            return input;
        }
        let lifted = input.get_mut("args").and_then(|a| a.as_object_mut()).and_then(|args| {
            ["command", "cmd"].iter().find_map(|k| args.remove(*k))
        });
        if let (Some(command), Some(obj)) = (lifted, input.as_object_mut()) {
            obj.insert("command".into(), command);
        }
        input
    }

    fn activity(&self, input: &serde_json::Value) -> String {
        display(input).unwrap_or_else(|| format!("using {}", self.service))
    }

    fn outcome(&self, input: &serde_json::Value) -> String {
        display(input).unwrap_or_else(|| format!("Used {}", self.service))
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let mut call: PluginCall = match serde_json::from_value(input) {
                Ok(c) => c,
                Err(e) => return ToolResult::error(format!("invalid input: {e}")),
            };
            call.slug = self.slug.clone();
            self.runner.run_command(ctx, &call).await
        })
    }
}

/// 3–8 words tool search scores: the service's name, its category and its
/// own trigger words.
fn search_hint(service: &str, manifest: Option<&napp::plugin::PluginManifest>) -> String {
    let mut candidates: Vec<String> = service.split_whitespace().map(str::to_string).collect();
    if let Some(m) = manifest {
        candidates.extend(m.category.split_whitespace().map(str::to_string));
        candidates.extend(m.triggers.iter().flat_map(|t| t.split_whitespace()).map(str::to_string));
    }
    candidates.extend(["service", "commands", "account"].map(str::to_string));
    let mut words: Vec<String> = Vec::new();
    for w in candidates {
        let w = w.trim().to_lowercase();
        if !w.is_empty() && !words.contains(&w) {
            words.push(w);
        }
    }
    // The fillers only make up a short hint.
    let own = words.iter().filter(|w| !["service", "commands", "account"].contains(&w.as_str())).count();
    words.truncate(own.clamp(3, 8));
    words.join(" ")
}

/// The model's approval sentence, when it wrote one.
fn display(input: &serde_json::Value) -> Option<String> {
    input
        .get("display")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_string)
}

/// `find_plugins`: search the marketplace; in a chat, the best match is an
/// install card the owner approves.
pub struct FindPluginsTool {
    runner: Arc<PluginRunner>,
}

impl FindPluginsTool {
    pub fn new(runner: Arc<PluginRunner>) -> Self {
        Self { runner }
    }
}

impl DynTool for FindPluginsTool {
    fn name(&self) -> &str {
        FIND_PLUGINS
    }

    fn description(&self) -> String {
        "Searches the marketplace for a tool that connects a service or adds a capability, and \
         offers the best match to the owner to install.\n\
         - When the owner asks for something no tool you have or can load does (post to a \
         service, look something up in an account), search here before saying it can't be done.\n\
         - In a chat the best match appears as an install card and this call waits for the \
         owner's answer; unattended, it lists what fits.\n\
         - Installed ones are already tools (plugin__<name>); don't search for those."
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The service's name or the job, e.g. \"invoices\"."
                }
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }

    fn search_hint(&self) -> &str {
        "marketplace install connect new service"
    }

    fn read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    /// It can park on the install card: a call that may ask runs alone.
    fn concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        false
    }

    fn activity(&self, _input: &serde_json::Value) -> String {
        "browsing the marketplace".to_string()
    }

    fn outcome(&self, _input: &serde_json::Value) -> String {
        "Browsed the marketplace".to_string()
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let query = input.get("query").and_then(|q| q.as_str()).unwrap_or_default();
            self.runner.handle_discover(query, ctx).await
        })
    }
}

/// `read_plugin_events`: the events an installed plugin can watch for.
pub struct ReadPluginEventsTool {
    runner: Arc<PluginRunner>,
}

impl ReadPluginEventsTool {
    pub fn new(runner: Arc<PluginRunner>) -> Self {
        Self { runner }
    }
}

impl DynTool for ReadPluginEventsTool {
    fn name(&self) -> &str {
        READ_PLUGIN_EVENTS
    }

    fn description(&self) -> String {
        "Lists the events an installed plugin can watch for, which an employee's automations \
         can start on."
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "plugin": {
                    "type": "string",
                    "description": "The installed plugin: its tool's name without plugin__."
                }
            },
            "required": ["plugin"],
            "additionalProperties": false
        })
    }

    fn search_hint(&self) -> &str {
        "plugin watch events automation triggers"
    }

    fn read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let raw = input.get("plugin").and_then(|p| p.as_str()).unwrap_or_default();
            let slug = plugin_slug(raw).unwrap_or(raw);
            if !self.runner.installed_slugs().iter().any(|s| s == slug) {
                return ToolResult::error(format!(
                    "No installed plugin is named {slug}. Installed plugins are the plugin__<name> tools."
                ));
            }
            self.runner.handle_events(slug)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Registry;

    /// `<root>/plugins/<slug>/0.1.0/` with a manifest, a stand-in binary and
    /// one skill per name.
    fn install(root: &std::path::Path, slug: &str, manifest: serde_json::Value, skills: &[&str]) {
        let dir = root.join("plugins").join(slug).join("0.1.0");
        std::fs::create_dir_all(&dir).unwrap();
        let mut m = serde_json::json!({"id": slug, "slug": slug, "name": slug, "version": "0.1.0", "platforms": {}});
        for (k, v) in manifest.as_object().unwrap() {
            m[k] = v.clone();
        }
        std::fs::write(dir.join("plugin.json"), m.to_string()).unwrap();
        std::fs::write(dir.join(slug), b"#!/bin/sh\necho ok\n").unwrap();
        for skill in skills {
            let sd = dir.join("skills").join(skill);
            std::fs::create_dir_all(&sd).unwrap();
            std::fs::write(sd.join("SKILL.md"), format!("---\nname: {skill}\ndescription: {skill}\n---\n")).unwrap();
        }
    }

    async fn registry(root: &std::path::Path) -> (Arc<Registry>, Arc<db::Store>) {
        std::fs::create_dir_all(root.join("plugins")).unwrap();
        std::fs::create_dir_all(root.join("user_plugins")).unwrap();
        let store = Arc::new(db::Store::new(root.join("t.db").to_str().unwrap()).unwrap());
        let registry = Arc::new(Registry::new(crate::gate::test_gate()));
        registry.set_plugin_store(Arc::new(napp::plugin::PluginStore::new(
            root.join("plugins"),
            root.join("user_plugins"),
            None,
        )));
        registry.register_all(store.clone(), crate::orchestrator::new_handle()).await;
        (registry, store)
    }

    /// One deferred tool per installed plugin, described by its own blurb
    /// and skills; its operations become tools of their own; a plugin that
    /// is removed or switched off takes its tools with it.
    #[tokio::test]
    async fn each_installed_plugin_is_its_own_tool_and_leaves_with_it() {
        let tmp = tempfile::tempdir().unwrap();
        let (registry, store) = registry(tmp.path()).await;
        assert!(registry.get(FIND_PLUGINS).await.is_some() && registry.get(READ_PLUGIN_EVENTS).await.is_some());
        assert!(registry.get_tool_names().await.iter().all(|n| plugin_slug(n).is_none()), "nothing installed yet");

        install(
            tmp.path(),
            "ledgerly",
            serde_json::json!({
                "name": "Ledgerly", "description": "Bookkeeping for small businesses.", "category": "accounting",
                "interfaceBindings": {"ledger.invoice.send": "invoice send {invoiceId}", "ledger.invoice.list": "invoice list"}
            }),
            &["ledgerly-invoice", "ledgerly-bill"],
        );
        registry.refresh_plugin_tools().await;
        let tool = registry.get("plugin__ledgerly").await.expect("the plugin's tool");
        assert!(registry.is_deferred("plugin__ledgerly").await);
        let d = tool.description();
        assert!(d.starts_with("Runs Ledgerly commands. Bookkeeping for small businesses."), "{d}");
        assert!(d.contains("use_skill") && d.contains("ledgerly-bill, ledgerly-invoice"), "{d}");
        assert!(d.contains("never here: ledger_invoice_send"), "the gated operation has its own tool: {d}");
        assert!(!d.contains("plugin("), "{d}");
        assert_eq!(tool.search_hint(), "ledgerly accounting service");
        for op in ["ledger_invoice_send", "ledger_invoice_list"] {
            assert!(registry.is_deferred(op).await, "{op}");
        }
        let ledger: std::collections::HashSet<String> = ["ledger_invoice_list", "ledger_invoice_send"].map(str::to_string).into();
        assert_eq!(registry.operation_tools_for(&["ledger".to_string()]).await, ledger);
        assert!(registry.operation_tools_for(&["mail".to_string()]).await.is_empty());

        store.upsert_installed_plugin("ledgerly", "Ledgerly", "0.1.0", "", "", "", "unverified").unwrap();
        store.set_plugin_enabled("ledgerly", false).unwrap();
        registry.refresh_plugin_tools().await;
        for gone in ["plugin__ledgerly", "ledger_invoice_send", "ledger_invoice_list"] {
            assert!(registry.get(gone).await.is_none(), "{gone} left with its plugin");
        }
        store.set_plugin_enabled("ledgerly", true).unwrap();
        registry.refresh_plugin_tools().await;
        assert!(registry.get("plugin__ledgerly").await.is_some());
        std::fs::remove_dir_all(tmp.path().join("plugins").join("ledgerly")).unwrap();
        registry.refresh_plugin_tools().await;
        assert!(registry.get("plugin__ledgerly").await.is_none() && registry.get("ledger_invoice_send").await.is_none());
    }

    /// An installed plugin that is not connected still has its tool (it is
    /// nameable and its skills readable) but performs no operation: an
    /// unconnected provider never claims an operation.
    #[tokio::test]
    async fn an_unconnected_plugin_has_its_tool_but_no_operations() {
        let tmp = tempfile::tempdir().unwrap();
        install(
            tmp.path(),
            "mailer",
            serde_json::json!({
                "auth": {"type": "env", "label": "Mailer key", "env": {"MAILER_KEY": ""}},
                "interfaceBindings": {"mail.message.send": "send"}
            }),
            &[],
        );
        let (registry, _store) = registry(tmp.path()).await;
        let tool = registry.get("plugin__mailer").await.expect("installed means nameable");
        assert!(tool.description().contains("Not connected yet"), "{}", tool.description());
        assert!(registry.get("mail_message_send").await.is_none());
    }

    #[test]
    fn a_nested_command_is_the_command() {
        let tmp = tempfile::tempdir().unwrap();
        let ps = Arc::new(napp::plugin::PluginStore::new(tmp.path().join("p"), tmp.path().join("u"), None));
        let db = Arc::new(db::Store::new(tmp.path().join("t.db").to_str().unwrap()).unwrap());
        let tool = PluginCliTool::new(Arc::new(PluginRunner::new(ps, db)), "ledgerly");
        let lifted = tool.normalize_input(serde_json::json!({"args": {"command": "doctor"}}));
        assert_eq!(lifted, serde_json::json!({"command": "doctor", "args": {}}));
        // An explicit command wins; args stay flags.
        let kept = serde_json::json!({"command": "query run", "args": {"command": "x", "query": "SELECT 1"}});
        assert_eq!(tool.normalize_input(kept.clone()), kept);
        // The owner reads the service's name, or the model's own sentence.
        assert_eq!(tool.activity(&serde_json::json!({"command": "doctor"})), "using Ledgerly");
        assert_eq!(tool.outcome(&serde_json::json!({"display": "Send invoice 1041"})), "Send invoice 1041");
        assert_eq!(tool.rule_key(&serde_json::json!({})), "plugin__ledgerly");
    }

    /// find_plugins may park on an install card, so it never runs beside
    /// other calls; read_plugin_events names only what is installed.
    #[tokio::test]
    async fn find_plugins_runs_alone_and_events_name_an_installed_plugin() {
        let tmp = tempfile::tempdir().unwrap();
        let (registry, _store) = registry(tmp.path()).await;
        let find = registry.get(FIND_PLUGINS).await.unwrap();
        assert!(find.read_only(&serde_json::json!({"query": "x"})));
        assert!(!find.concurrency_safe(&serde_json::json!({"query": "x"})));
        let r = registry
            .execute(&ToolContext::default(), READ_PLUGIN_EVENTS, serde_json::json!({"plugin": "nothing"}))
            .await;
        assert!(r.is_error && r.content.contains("No installed plugin is named nothing"), "{}", r.content);
    }
}
