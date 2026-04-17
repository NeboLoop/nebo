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
    build_domain_description, build_domain_schema, DomainSchemaConfig, FieldConfig, ResourceConfig,
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
                "Manage A2UI interactive surfaces for rich agent UIs. Surfaces render interactive components (text, buttons, inputs, etc.) in the Nebo frontend."
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

    fn requires_approval(&self) -> bool {
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
                None => return ToolResult::error("action is required"),
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
    fn resolve_agent_id<'a>(params: &'a serde_json::Value, ctx: &'a ToolContext) -> &'a str {
        let explicit = params.get("agent_id").and_then(|v| v.as_str()).unwrap_or("");
        if !explicit.is_empty() {
            return explicit;
        }
        // Derive from session key: "agent:{id}:{channel}"
        if ctx.session_key.starts_with("agent:") {
            if let Some(id) = ctx.session_key.split(':').nth(1) {
                if !id.is_empty() {
                    return id;
                }
            }
        }
        ""
    }

    async fn handle_surface(&self, action: &str, params: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
        match action {
            "create" => {
                let agent_id = Self::resolve_agent_id(params, ctx);
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
                    return ToolResult::error("agent_id is required (pass it explicitly or ensure session is agent-scoped)");
                }

                match self
                    .host
                    .create_surface(agent_id, view_id, surface_type, catalog_id, theme)
                    .await
                {
                    Ok(sid) => {
                        debug!("a2ui: created surface {}", sid);
                        ToolResult::ok(
                            json!({ "surface_id": sid, "status": "created" }).to_string(),
                        )
                    }
                    Err(e) => ToolResult::error(format!("Failed to create surface: {e}")),
                }
            }

            "update_components" => {
                let surface_id = match params.get("surface_id").and_then(|v| v.as_str()) {
                    Some(id) => id,
                    None => return ToolResult::error("surface_id is required"),
                };
                let components = match params.get("components").and_then(|v| v.as_array()) {
                    Some(arr) => arr.clone(),
                    None => return ToolResult::error("components array is required"),
                };

                match self.host.update_components(surface_id, components).await {
                    Ok(()) => ToolResult::ok(
                        json!({ "surface_id": surface_id, "status": "components_updated" })
                            .to_string(),
                    ),
                    Err(e) => ToolResult::error(format!("Failed to update components: {e}")),
                }
            }

            "update_data" => {
                let surface_id = match params.get("surface_id").and_then(|v| v.as_str()) {
                    Some(id) => id,
                    None => return ToolResult::error("surface_id is required"),
                };
                let path = params.get("path").and_then(|v| v.as_str());
                let value = match params.get("value") {
                    Some(v) => v.clone(),
                    None => return ToolResult::error("value is required"),
                };

                match self.host.update_data_model(surface_id, path, value).await {
                    Ok(()) => ToolResult::ok(
                        json!({ "surface_id": surface_id, "status": "data_updated" }).to_string(),
                    ),
                    Err(e) => ToolResult::error(format!("Failed to update data: {e}")),
                }
            }

            "navigate" => {
                let surface_id = params.get("surface_id").and_then(|v| v.as_str()).unwrap_or("");
                let agent_id_param = Self::resolve_agent_id(params, ctx);
                let target_view = match params.get("target_view").and_then(|v| v.as_str()) {
                    Some(v) => v,
                    None => return ToolResult::error("target_view is required"),
                };
                let nav_params = params.get("params").cloned();
                let views_json = match params.get("views_json") {
                    Some(v) => v.clone(),
                    None => return ToolResult::error("views_json is required (agent views definition)"),
                };

                // Derive agent_id and from_view from surface_id or params
                let (agent_id, from_view) = if !surface_id.is_empty() {
                    let parts: Vec<&str> = surface_id.split(':').collect();
                    let aid = if parts.len() >= 2 { parts[1] } else { agent_id_param };
                    let fv = if parts.len() >= 3 { parts[2] } else { "default" };
                    (aid, fv)
                } else {
                    (agent_id_param, "default")
                };

                if agent_id.is_empty() {
                    return ToolResult::error("agent_id is required for navigate");
                }

                match self.host.navigate_view(agent_id, from_view, target_view, nav_params, &views_json).await {
                    Ok(sid) => ToolResult::ok(
                        json!({ "surface_id": sid, "status": "navigated", "view": target_view }).to_string(),
                    ),
                    Err(e) => ToolResult::error(format!("Failed to navigate: {e}")),
                }
            }

            "delete" => {
                let surface_id = match params.get("surface_id").and_then(|v| v.as_str()) {
                    Some(id) => id,
                    None => return ToolResult::error("surface_id is required"),
                };

                match self.host.delete_surface(surface_id).await {
                    Ok(()) => ToolResult::ok(
                        json!({ "surface_id": surface_id, "status": "deleted" }).to_string(),
                    ),
                    Err(e) => ToolResult::error(format!("Failed to delete surface: {e}")),
                }
            }

            "list" => {
                let agent_id = Self::resolve_agent_id(params, ctx);
                if agent_id.is_empty() {
                    return ToolResult::error("agent_id is required (pass it explicitly or ensure session is agent-scoped)");
                }

                let surfaces = self.host.list_surfaces(agent_id).await;
                ToolResult::ok(serde_json::to_string(&surfaces).unwrap_or_default())
            }

            other => ToolResult::error(format!(
                "Unknown action: {other}. Use: create, update_components, update_data, delete, list"
            )),
        }
    }
}
