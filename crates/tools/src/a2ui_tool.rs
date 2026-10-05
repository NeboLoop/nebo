//! A2UI domain tool — lets agents create and manage interactive surfaces.
//!
//! STRAP pattern: `a2ui(resource, action, ...params)`
//!
//! Resources:
//!   - surface: create, update_components, update_data, delete, list
//!
//! The tool delegates to an `A2UIHost` trait, injected at startup from the server crate.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::Serialize;
use serde_json::json;
use tracing::debug;

use crate::domain::{
    DomainSchemaConfig, FieldConfig, ResourceConfig, build_domain_description, build_domain_schema,
};
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

// ---------------------------------------------------------------------------
// A2UIHost trait — implemented in server crate, injected via late binding
// ---------------------------------------------------------------------------

/// Trait for the A2UI surface manager. Implemented by server::a2ui::A2UIManager.
pub trait A2UIHost: Send + Sync {
    fn create_surface(
        &self,
        agent_id: &str,
        view_id: &str,
        surface_type: &str,
        catalog_id: &str,
        theme: Option<serde_json::Value>,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>>;

    fn update_components(
        &self,
        surface_id: &str,
        components: Vec<serde_json::Value>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;

    fn update_data_model(
        &self,
        surface_id: &str,
        path: Option<&str>,
        value: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;

    fn delete_surface(
        &self,
        surface_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;

    fn list_surfaces(
        &self,
        agent_id: &str,
    ) -> Pin<Box<dyn Future<Output = Vec<SurfaceSummary>> + Send + '_>>;

    fn navigate_view(
        &self,
        agent_id: &str,
        from_view: &str,
        to_view: &str,
        params: Option<serde_json::Value>,
        views_json: &serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>>;
}

/// Summary of an active surface (returned by list).
#[derive(Debug, Clone, Serialize)]
pub struct SurfaceSummary {
    pub surface_id: String,
    pub view_id: String,
    pub surface_type: String,
}

// ---------------------------------------------------------------------------
// Who sees a surface
// ---------------------------------------------------------------------------

/// `a2ui` is withheld from every employee that is not an app: only an app's
/// own page draws A2UI surfaces. The chat on desktop and phone has no
/// renderer, so for any other employee the tool drew panels nobody saw — Chief
/// told the owner "a card panel on the side" that never appeared, then took
/// his "k" for option A (live 2026-10-02). A question to the owner is
/// `ask_owner`'s card, which both apps show.
pub fn withheld(store: &db::Store, agent_id: &str) -> Vec<String> {
    if crate::app_data::is_app(store, agent_id) {
        Vec::new()
    } else {
        vec!["a2ui".to_string()]
    }
}

/// Whether the owner can see a surface `agent_id` draws in this run, or why
/// nothing appeared. Only an open page of the app draws one; a voice call
/// has no screen and a messaging channel shows text.
fn sight(ctx: &ToolContext, agent_id: &str) -> Result<(), &'static str> {
    if ctx.door == types::permissions::Door::Voice {
        return Err("the owner is on a voice call, which has no screen");
    }
    if ctx.channel.is_some() {
        return Err("the owner is writing from a messaging channel, which shows only text");
    }
    if !crate::app_dev::has_open_view(agent_id) {
        return Err("no page of your app is open, and the chat on desktop and in the mobile app can't draw A2UI surfaces");
    }
    Ok(())
}

/// What the model is told when nothing was shown.
pub const NOT_SHOWN: &str = "Nothing appeared on the owner's screen: never say a panel or card is showing. To have \
him pick or answer, call ask_owner (its options are buttons in his chat); otherwise say it in your reply.";

/// A drawing call's result: what was done, and plainly whether the owner
/// sees it.
fn drawn(mut done: serde_json::Value, sight: Result<(), &'static str>) -> ToolResult {
    match sight {
        Ok(()) => {
            done["shown"] = json!("on your app's open page, if the page draws A2UI surfaces");
            done["answers"] = json!(
                "What the owner presses there comes back to you as a message. Until one does, nothing is chosen: a typed reply is a message, never a pick."
            );
        }
        Err(why) => {
            done["shown"] = json!(false);
            done["why"] = json!(why);
            done["next"] = json!(NOT_SHOWN);
        }
    }
    ToolResult::ok(done.to_string())
}

// ---------------------------------------------------------------------------
// A2UIDomainTool
// ---------------------------------------------------------------------------

pub struct A2UIDomainTool {
    host: Arc<dyn A2UIHost>,
}

impl A2UIDomainTool {
    pub fn new(host: Arc<dyn A2UIHost>) -> Self {
        Self { host }
    }

    fn domain_config() -> DomainSchemaConfig {
        let mut resources = HashMap::new();
        resources.insert(
            "surface".to_string(),
            ResourceConfig {
                name: "surface".to_string(),
                actions: vec![
                    "create".into(),
                    "update_components".into(),
                    "update_data".into(),
                    "navigate".into(),
                    "delete".into(),
                    "list".into(),
                ],
                description: "A2UI rendering surface (panel, window, overlay)".into(),
            },
        );

        DomainSchemaConfig {
            domain: "a2ui".to_string(),
            description:
                "Draws A2UI surfaces (text, buttons, inputs) on your app's own page. The chat on desktop and in the mobile app can't show them, and the result says whether anything was shown. To ask the owner a question or have him pick, use ask_owner."
                    .to_string(),
            resources,
            fields: vec![
                FieldConfig {
                    name: "agent_id".into(),
                    field_type: "string".into(),
                    description: "Agent that owns the surface (auto-resolved from session if omitted)".into(),
                    required: false,
                    enum_values: vec![],
                    default: None,
                },
                FieldConfig {
                    name: "view_id".into(),
                    field_type: "string".into(),
                    description: "View identifier from views.json".into(),
                    required: false,
                    enum_values: vec![],
                    default: Some(json!("default")),
                },
                FieldConfig {
                    name: "surface_id".into(),
                    field_type: "string".into(),
                    description: "Target surface ID (agent:{agent_id}:{view_id})".into(),
                    required: false,
                    enum_values: vec![],
                    default: None,
                },
                FieldConfig {
                    name: "surface_type".into(),
                    field_type: "string".into(),
                    description: "Surface display mode".into(),
                    required: false,
                    enum_values: vec![
                        "panel".into(),
                        "window".into(),
                        "overlay".into(),
                    ],
                    default: Some(json!("panel")),
                },
                FieldConfig {
                    name: "catalog_id".into(),
                    field_type: "string".into(),
                    description: "Component catalog to use".into(),
                    required: false,
                    enum_values: vec![],
                    default: Some(json!("https://a2ui.org/specification/v0_9/basic_catalog.json")),
                },
                FieldConfig {
                    name: "theme".into(),
                    field_type: "object".into(),
                    description: "Optional theme overrides".into(),
                    required: false,
                    enum_values: vec![],
                    default: None,
                },
                FieldConfig {
                    name: "components".into(),
                    field_type: "array".into(),
                    description: "Flat adjacency list of A2UI components".into(),
                    required: false,
                    enum_values: vec![],
                    default: None,
                },
                FieldConfig {
                    name: "path".into(),
                    field_type: "string".into(),
                    description: "JSON Pointer path for data model update (e.g. /users/0/name)"
                        .into(),
                    required: false,
                    enum_values: vec![],
                    default: None,
                },
                FieldConfig {
                    name: "value".into(),
                    field_type: "string".into(),
                    description: "Value for data model update (any JSON value)".into(),
                    required: false,
                    enum_values: vec![],
                    default: None,
                },
                FieldConfig {
                    name: "target_view".into(),
                    field_type: "string".into(),
                    description: "Target view ID for navigate action".into(),
                    required: false,
                    enum_values: vec![],
                    default: None,
                },
                FieldConfig {
                    name: "params".into(),
                    field_type: "object".into(),
                    description: "Parameters to pass to the target view (injected as data model)".into(),
                    required: false,
                    enum_values: vec![],
                    default: None,
                },
                FieldConfig {
                    name: "views_json".into(),
                    field_type: "object".into(),
                    description: "Views definition object (auto-resolved from agent if omitted)".into(),
                    required: false,
                    enum_values: vec![],
                    default: None,
                },
            ],
            examples: vec![
                r#"a2ui(resource: "surface", action: "create", agent_id: "crm", view_id: "dashboard")"#.into(),
                r#"a2ui(action: "update_components", surface_id: "agent:crm:dashboard", components: [...])"#.into(),
                r#"a2ui(action: "update_data", surface_id: "agent:crm:dashboard", path: "/title", value: "My CRM")"#.into(),
                r#"a2ui(action: "delete", surface_id: "agent:crm:dashboard")"#.into(),
                r#"a2ui(action: "navigate", surface_id: "agent:crm:dashboard", target_view: "settings", params: {"tab": "general"})"#.into(),
                r#"a2ui(action: "list", agent_id: "crm")"#.into(),
            ],
        }
    }
}

impl DynTool for A2UIDomainTool {
    fn name(&self) -> &str {
        "a2ui"
    }

    fn description(&self) -> String {
        build_domain_description(&Self::domain_config())
    }

    fn schema(&self) -> serde_json::Value {
        build_domain_schema(&Self::domain_config())
    }


    fn search_hint(&self) -> &str {
        "interactive views surfaces ui components"
    }

    fn read_only(&self, input: &serde_json::Value) -> bool {
        input.get("action").and_then(|v| v.as_str()) == Some("list")
    }

    fn rule_key(&self, input: &serde_json::Value) -> String {
        match input.get("action").and_then(|v| v.as_str()).unwrap_or("") {
            "update_components" => "update_view_components",
            "update_data" => "update_view_data",
            "navigate" => "navigate_view",
            "delete" => "delete_view",
            "list" => "list_views",
            _ => "create_view",
        }
        .to_string()
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
    ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let resource = input
                .get("resource")
                .and_then(|v| v.as_str())
                .unwrap_or("surface");
            let action = match input.get("action").and_then(|v| v.as_str()) {
                Some(a) => a,
                None => return ToolResult::error(crate::errors::missing_param(
                    "a2ui",
                    "action",
                    "a2ui(resource: \"surface\", action: \"create\", components: [...])\nAvailable actions: create, update_components, update_data, navigate, delete, list",
                )),
            };

            match resource {
                "surface" => self.handle_surface(action, &input, ctx).await,
                other => ToolResult::error(format!("Unknown resource: {other}. Use: surface")),
            }
        })
    }
}

impl A2UIDomainTool {
    /// Resolve agent_id: use explicit param if provided, otherwise derive from
    /// ToolContext.session_key (format "agent:{id}:{channel}").
    fn resolve_agent_id(params: &serde_json::Value, ctx: &ToolContext) -> String {
        let explicit = params
            .get("agent_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !explicit.is_empty() {
            return explicit.to_string();
        }
        // Canonical, subagent-aware extraction (CODE_AUDITOR Rule 8).
        types::keyparser::extract_agent_id(&ctx.session_key)
    }

    /// The agent a surface id names (`agent:{id}:{view}`), or the session's.
    fn surface_agent(surface_id: &str, params: &serde_json::Value, ctx: &ToolContext) -> String {
        types::keyparser::agent_id_from_surface_id(surface_id)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| Self::resolve_agent_id(params, ctx))
    }

    async fn handle_surface(
        &self,
        action: &str,
        params: &serde_json::Value,
        ctx: &ToolContext,
    ) -> ToolResult {
        match action {
            "create" => {
                let agent_id_owned = Self::resolve_agent_id(params, ctx);
                let agent_id = agent_id_owned.as_str();
                let view_id = params
                    .get("view_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default");
                let surface_type = params
                    .get("surface_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("panel");
                let catalog_id = params
                    .get("catalog_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("https://a2ui.org/specification/v0_9/basic_catalog.json");
                let theme = params.get("theme").cloned();

                if agent_id.is_empty() {
                    return ToolResult::error(
                        "agent_id is required (pass it explicitly or ensure session is agent-scoped). Example: a2ui(action: \"create\", agent_id: \"crm\", view_id: \"dashboard\")",
                    );
                }

                let sid = match self
                    .host
                    .create_surface(agent_id, view_id, surface_type, catalog_id, theme)
                    .await
                {
                    Ok(sid) => sid,
                    Err(e) => return ToolResult::error(format!("Failed to create surface: {e}")),
                };
                debug!("a2ui: created surface {}", sid);
                // Components given with the create are its first content: a
                // create that dropped them left an empty surface (live
                // 2026-10-02).
                if let Some(components) = params.get("components").and_then(|v| v.as_array()) {
                    if let Err(e) = self.host.update_components(&sid, components.clone()).await {
                        return ToolResult::error(format!("Created {sid}, but its components failed: {e}"));
                    }
                }
                drawn(json!({ "surface_id": sid, "status": "created" }), sight(ctx, agent_id))
            }

            "update_components" => {
                let surface_id = match params.get("surface_id").and_then(|v| v.as_str()) {
                    Some(id) => id,
                    None => return ToolResult::error(crate::errors::missing_param(
                        "update_components",
                        "surface_id",
                        "a2ui(resource: \"surface\", action: \"update_components\", surface_id: \"agent:crm:dashboard\", components: [...])",
                    )),
                };
                let components = match params.get("components").and_then(|v| v.as_array()) {
                    Some(arr) => arr.clone(),
                    None => return ToolResult::error(crate::errors::missing_param(
                        "update_components",
                        "components",
                        "a2ui(resource: \"surface\", action: \"update_components\", surface_id: \"agent:crm:dashboard\", components: [...])",
                    )),
                };

                match self.host.update_components(surface_id, components).await {
                    Ok(()) => drawn(
                        json!({ "surface_id": surface_id, "status": "components_updated" }),
                        sight(ctx, &Self::surface_agent(surface_id, params, ctx)),
                    ),
                    Err(e) => ToolResult::error(format!("Failed to update components: {e}")),
                }
            }

            "update_data" => {
                let surface_id = match params.get("surface_id").and_then(|v| v.as_str()) {
                    Some(id) => id,
                    None => return ToolResult::error(crate::errors::missing_param(
                        "update_data",
                        "surface_id",
                        "a2ui(resource: \"surface\", action: \"update_data\", surface_id: \"agent:crm:dashboard\", value: {...})",
                    )),
                };
                let path = params.get("path").and_then(|v| v.as_str());
                let value = match params.get("value") {
                    Some(v) => v.clone(),
                    None => return ToolResult::error(crate::errors::missing_param(
                        "update_data",
                        "value",
                        "a2ui(resource: \"surface\", action: \"update_data\", surface_id: \"agent:crm:dashboard\", value: {\"count\": 42})",
                    )),
                };

                match self.host.update_data_model(surface_id, path, value).await {
                    Ok(()) => drawn(
                        json!({ "surface_id": surface_id, "status": "data_updated" }),
                        sight(ctx, &Self::surface_agent(surface_id, params, ctx)),
                    ),
                    Err(e) => ToolResult::error(format!("Failed to update data: {e}")),
                }
            }

            "navigate" => {
                let surface_id = params
                    .get("surface_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let agent_id_param_owned = Self::resolve_agent_id(params, ctx);
                let agent_id_param = agent_id_param_owned.as_str();
                let target_view = match params.get("target_view").and_then(|v| v.as_str()) {
                    Some(v) => v,
                    None => return ToolResult::error(crate::errors::missing_param(
                        "navigate",
                        "target_view",
                        "a2ui(resource: \"surface\", action: \"navigate\", target_view: \"settings\", views_json: {...})",
                    )),
                };
                let nav_params = params.get("params").cloned();
                let views_json = match params.get("views_json") {
                    Some(v) => v.clone(),
                    None => {
                        return ToolResult::error(
                            "views_json is required (agent views definition)",
                        );
                    }
                };

                // Derive agent_id and from_view from surface_id or params
                let (agent_id, from_view) = if !surface_id.is_empty() {
                    let parts: Vec<&str> = surface_id.split(':').collect();
                    let aid = if parts.len() >= 2 {
                        parts[1]
                    } else {
                        agent_id_param
                    };
                    let fv = if parts.len() >= 3 {
                        parts[2]
                    } else {
                        "default"
                    };
                    (aid, fv)
                } else {
                    (agent_id_param, "default")
                };

                if agent_id.is_empty() {
                    return ToolResult::error(crate::errors::missing_param(
                        "navigate",
                        "agent_id",
                        "a2ui(resource: \"surface\", action: \"navigate\", agent_id: \"abc\", target_view: \"settings\", views_json: {...})",
                    ));
                }

                match self
                    .host
                    .navigate_view(agent_id, from_view, target_view, nav_params, &views_json)
                    .await
                {
                    Ok(sid) => drawn(
                        json!({ "surface_id": sid, "status": "navigated", "view": target_view }),
                        sight(ctx, agent_id),
                    ),
                    Err(e) => ToolResult::error(format!("Failed to navigate: {e}")),
                }
            }

            "delete" => {
                let surface_id = match params.get("surface_id").and_then(|v| v.as_str()) {
                    Some(id) => id,
                    None => return ToolResult::error(crate::errors::missing_param(
                        "delete",
                        "surface_id",
                        "a2ui(resource: \"surface\", action: \"delete\", surface_id: \"agent:crm:dashboard\")",
                    )),
                };

                match self.host.delete_surface(surface_id).await {
                    Ok(()) => ToolResult::ok(
                        json!({ "surface_id": surface_id, "status": "deleted" }).to_string(),
                    ),
                    Err(e) => ToolResult::error(format!("Failed to delete surface: {e}")),
                }
            }

            "list" => {
                let agent_id_owned = Self::resolve_agent_id(params, ctx);
                let agent_id = agent_id_owned.as_str();
                if agent_id.is_empty() {
                    return ToolResult::error(
                        "agent_id is required (pass it explicitly or ensure session is agent-scoped). Example: a2ui(action: \"create\", agent_id: \"crm\", view_id: \"dashboard\")",
                    );
                }

                let surfaces = self.host.list_surfaces(agent_id).await;
                ToolResult::ok(serde_json::to_string(&surfaces).unwrap_or_default())
            }

            other => ToolResult::error(format!(
                "Unknown action: {other}. Use: create, update_components, update_data, navigate, delete, list"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A host that does what it is told and keeps the components it got.
    #[derive(Default)]
    struct Host {
        components: Mutex<Vec<(String, usize)>>,
    }

    impl A2UIHost for Host {
        fn create_surface(&self, agent_id: &str, view_id: &str, _: &str, _: &str, _: Option<serde_json::Value>) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
            let sid = format!("agent:{agent_id}:{view_id}");
            Box::pin(async move { Ok(sid) })
        }
        fn update_components(&self, surface_id: &str, components: Vec<serde_json::Value>) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
            self.components.lock().unwrap().push((surface_id.to_string(), components.len()));
            Box::pin(async { Ok(()) })
        }
        fn update_data_model(&self, _: &str, _: Option<&str>, _: serde_json::Value) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
            Box::pin(async { Ok(()) })
        }
        fn delete_surface(&self, _: &str) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
            Box::pin(async { Ok(()) })
        }
        fn list_surfaces(&self, _: &str) -> Pin<Box<dyn Future<Output = Vec<SurfaceSummary>> + Send + '_>> {
            Box::pin(async { Vec::new() })
        }
        fn navigate_view(&self, agent_id: &str, _: &str, to: &str, _: Option<serde_json::Value>, _: &serde_json::Value) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
            let sid = format!("agent:{agent_id}:{to}");
            Box::pin(async move { Ok(sid) })
        }
    }

    fn ctx(agent: &str, door: types::permissions::Door) -> ToolContext {
        ToolContext {
            session_key: format!("agent:{agent}:web"),
            door,
            ..Default::default()
        }
    }

    /// Chief's call, as he made it (live 2026-10-02): a choice drawn with
    /// `create`, components and all.
    fn chiefs_panel() -> serde_json::Value {
        json!({"action": "create", "view_id": "interview", "surface_type": "panel", "components": [
            {"id": "q1-title", "type": "text", "props": {"content": "1. What's the most important thing to focus on right now?"}},
            {"id": "q1-opt-a", "type": "button", "props": {"content": "Finishing pending invoicing and getting paid"}},
            {"id": "q1-opt-b", "type": "button", "props": {"content": "Marketing that's been running this month"}}
        ]})
    }

    fn read(r: &ToolResult) -> serde_json::Value {
        assert!(!r.is_error, "{}", r.content);
        serde_json::from_str(&r.content).unwrap()
    }

    /// Only an app's page draws A2UI: every other employee is never offered
    /// the tool, and a question to the owner is ask_owner's card.
    #[test]
    fn only_an_app_employee_is_offered_a2ui() {
        let dir = tempfile::tempdir().unwrap();
        let store = db::Store::new(&dir.path().join("t.db").to_string_lossy()).unwrap();
        store.create_agent("chief", Some("user"), "Chief", "", "---\nname: x\n---\n", "{}", None, None).unwrap();
        store.create_agent("crm", Some("user"), "CRM", "", "---\nname: x\n---\n", "{}", None, None).unwrap();
        store.set_agent_app_fields("crm", true, Some("/tmp/ui"), None, None).unwrap();
        assert_eq!(withheld(&store, "chief"), vec!["a2ui".to_string()]);
        assert_eq!(withheld(&store, ""), vec!["a2ui".to_string()]);
        assert!(withheld(&store, "crm").is_empty());
        let d = A2UIDomainTool::new(Arc::new(Host::default())).description();
        assert!(d.contains("The chat on desktop and in the mobile app can't show them") && d.contains("use ask_owner"), "{d}");
    }

    /// Where nothing can draw the surface — no page of the app open, a voice
    /// call, a messaging channel — the result says plainly that nothing was
    /// shown and sends the employee to ask_owner; it never reads as shown.
    #[tokio::test]
    async fn a_surface_nobody_can_see_is_said_to_be_unseen() {
        let tool = A2UIDomainTool::new(Arc::new(Host::default()));
        let unseen = [
            (ctx("chief-unseen", types::permissions::Door::Chat), "no page of your app is open"),
            (ctx("chief-unseen", types::permissions::Door::Voice), "voice call"),
            (
                ToolContext { channel: Some(Default::default()), ..ctx("chief-unseen", types::permissions::Door::Chat) },
                "messaging channel",
            ),
        ];
        for (ctx, why) in unseen {
            for input in [
                chiefs_panel(),
                json!({"action": "update_components", "surface_id": "agent:chief-unseen:interview", "components": [{"id": "t", "type": "text", "props": {"content": "Question 2"}}]}),
                json!({"action": "update_data", "surface_id": "agent:chief-unseen:interview", "path": "/answers/q1", "value": "b"}),
            ] {
                let v = read(&tool.execute_dyn(&ctx, input.clone()).await);
                assert_eq!(v["shown"], false, "{input}: {v}");
                assert!(v["why"].as_str().unwrap().contains(why), "{v}");
                assert_eq!(v["next"], NOT_SHOWN);
                assert!(NOT_SHOWN.contains("never say a panel or card is showing") && NOT_SHOWN.contains("call ask_owner"));
            }
        }
    }

    /// On an app whose page is open, the surface is drawn there, and the
    /// components a create carries are its content.
    #[tokio::test]
    async fn an_open_app_page_shows_the_surface_with_the_components_it_was_created_with() {
        let host = Arc::new(Host::default());
        let tool = A2UIDomainTool::new(host.clone());
        let _page = crate::app_dev::view_opened("crm-open-page");
        let v = read(&tool.execute_dyn(&ctx("crm-open-page", types::permissions::Door::Chat), chiefs_panel()).await);
        assert_eq!(v["surface_id"], "agent:crm-open-page:interview");
        assert!(v["shown"].as_str().unwrap().contains("open page"), "{v}");
        assert!(v["answers"].as_str().unwrap().contains("a typed reply is a message, never a pick"), "{v}");
        assert_eq!(*host.components.lock().unwrap(), vec![("agent:crm-open-page:interview".to_string(), 3)]);
    }
}
