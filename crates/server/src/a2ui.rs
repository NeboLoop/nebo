//! A2UI surface management and catalog provisioning.
//!
//! This module implements Nebo's A2UI host infrastructure:
//! - `NeboCatalogProvider` — serves the basic A2UI catalog to agents
//! - `A2UIManager` — manages surface lifecycle and broadcasts A2UI messages to clients

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use a2ui_core::traits::{CatalogInfo, CatalogProvider};
use a2ui_types::common::CatalogId;
use a2ui_types::v09::catalog::Catalog;
use serde_json::json;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use db::Store;

use crate::handlers::ws::ClientHub;

// ---------------------------------------------------------------------------
// NeboCatalogProvider
// ---------------------------------------------------------------------------

/// Serves A2UI component catalogs to the agent runner and prompt builder.
///
/// Phase 1 ships only the basic catalog. Phase 5 will add a custom Nebo catalog.
pub struct NeboCatalogProvider {
    catalogs: HashMap<String, (Catalog, serde_json::Value)>,
}

impl NeboCatalogProvider {
    /// Create a provider with the built-in basic catalog.
    pub fn new() -> Self {
        let basic_id = "https://a2ui.org/specification/v0_9/basic_catalog.json";
        let basic_catalog = Self::build_basic_catalog();
        let basic_schema = Self::build_basic_schema();

        let mut catalogs = HashMap::new();
        catalogs.insert(basic_id.to_string(), (basic_catalog, basic_schema));

        Self { catalogs }
    }

    /// Build the basic A2UI catalog with standard component types.
    /// All 18 types matching @a2ui/lit v0.9.
    fn build_basic_catalog() -> Catalog {
        serde_json::from_value(json!({
            "catalogId": "https://a2ui.org/specification/v0_9/basic_catalog.json",
            "components": {
                "text": {
                    "type": "object",
                    "properties": {
                        "content": { "type": "string" },
                        "variant": { "type": "string", "enum": ["body", "h1", "h2", "h3", "h4", "h5", "caption", "overline"] }
                    },
                    "required": ["content"]
                },
                "heading": {
                    "type": "object",
                    "properties": {
                        "content": { "type": "string" },
                        "level": { "type": "integer", "minimum": 1, "maximum": 6 }
                    },
                    "required": ["content"]
                },
                "button": {
                    "type": "object",
                    "properties": {
                        "label": { "type": "string" },
                        "action": { "type": "object" },
                        "variant": { "type": "string", "enum": ["primary", "secondary", "ghost", "danger"] }
                    },
                    "required": ["label"]
                },
                "input": {
                    "type": "object",
                    "properties": {
                        "label": { "type": "string" },
                        "placeholder": { "type": "string" },
                        "value": { "type": "string" }
                    }
                },
                "select": {
                    "type": "object",
                    "properties": {
                        "label": { "type": "string" },
                        "options": { "type": "array", "items": { "type": "object" } },
                        "value": { "type": "string" }
                    }
                },
                "choicePicker": {
                    "type": "object",
                    "properties": {
                        "label": { "type": "string" },
                        "options": { "type": "array", "items": { "type": "object", "properties": { "label": { "type": "string" }, "value": { "type": "string" } } } },
                        "value": { "type": "string" }
                    }
                },
                "toggle": {
                    "type": "object",
                    "properties": {
                        "label": { "type": "string" },
                        "value": { "type": "boolean" }
                    }
                },
                "checkbox": {
                    "type": "object",
                    "properties": {
                        "label": { "type": "string" },
                        "checked": { "type": "boolean" }
                    },
                    "required": ["label"]
                },
                "slider": {
                    "type": "object",
                    "properties": {
                        "label": { "type": "string" },
                        "min": { "type": "number" },
                        "max": { "type": "number" },
                        "step": { "type": "number" },
                        "value": { "type": "number" }
                    }
                },
                "divider": {
                    "type": "object",
                    "properties": {}
                },
                "image": {
                    "type": "object",
                    "properties": {
                        "src": { "type": "string" },
                        "alt": { "type": "string" }
                    },
                    "required": ["src"]
                },
                "list": {
                    "type": "object",
                    "properties": {
                        "items": { "type": "array", "items": { "type": "object", "properties": { "label": { "type": "string" }, "secondary": { "type": "string" } } } }
                    },
                    "required": ["items"]
                },
                "tabs": {
                    "type": "object",
                    "properties": {
                        "tabs": { "type": "array", "items": { "type": "object", "properties": { "label": { "type": "string" }, "id": { "type": "string" } } } },
                        "activeTab": { "type": "string" }
                    },
                    "required": ["tabs"]
                },
                "card": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "children": { "type": "array", "items": { "type": "string" } }
                    }
                },
                "column": {
                    "type": "object",
                    "properties": {
                        "children": { "type": "array", "items": { "type": "string" } },
                        "gap": { "type": "string" }
                    }
                },
                "row": {
                    "type": "object",
                    "properties": {
                        "children": { "type": "array", "items": { "type": "string" } },
                        "gap": { "type": "string" }
                    }
                },
                "icon": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "size": { "type": "string" }
                    },
                    "required": ["name"]
                },
                "modal": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "open": { "type": "boolean" },
                        "children": { "type": "array", "items": { "type": "string" } }
                    }
                }
            }
        }))
        .expect("basic catalog JSON is valid")
    }

    /// Build the JSON Schema for the basic catalog (used for validation and LLM prompts).
    fn build_basic_schema() -> serde_json::Value {
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "title": "Basic A2UI Catalog",
            "description": "Standard component types for A2UI v0.9 surfaces",
            "type": "object",
            "properties": {
                "text": { "description": "Display text content (variant: body, h1-h5, caption, overline)" },
                "heading": { "description": "Section heading (h1-h6) — alias for text with heading variant" },
                "button": { "description": "Clickable button with action (variant: primary, secondary, ghost, danger)" },
                "input": { "description": "Text input field" },
                "select": { "description": "Dropdown selector — alias for choicePicker" },
                "choicePicker": { "description": "Selection dropdown with label/value options" },
                "toggle": { "description": "Boolean toggle switch — alias for checkbox" },
                "checkbox": { "description": "Boolean checkbox with label" },
                "slider": { "description": "Numeric slider with min/max/step" },
                "divider": { "description": "Visual separator" },
                "image": { "description": "Image display" },
                "list": { "description": "List of items with label and optional secondary text" },
                "tabs": { "description": "Tabbed container with tab labels and IDs" },
                "card": { "description": "Card container with optional title" },
                "column": { "description": "Vertical layout container" },
                "row": { "description": "Horizontal layout container" },
                "icon": { "description": "Icon display by name" },
                "modal": { "description": "Modal dialog overlay" }
            }
        })
    }
}

impl CatalogProvider for NeboCatalogProvider {
    fn available_catalogs(&self) -> Vec<CatalogInfo> {
        self.catalogs
            .keys()
            .map(|id| CatalogInfo {
                catalog_id: CatalogId::new(id.clone()),
                description: Some(format!("{} catalog", id)),
            })
            .collect()
    }

    fn get_catalog(&self, id: &CatalogId) -> Option<Catalog> {
        self.catalogs.get(id.as_str()).map(|(cat, _)| cat.clone())
    }

    fn get_catalog_schema(&self, id: &CatalogId) -> Option<serde_json::Value> {
        self.catalogs
            .get(id.as_str())
            .map(|(_, schema)| schema.clone())
    }
}

// ---------------------------------------------------------------------------
// A2UIManager
// ---------------------------------------------------------------------------

/// In-memory state for a single active surface.
#[derive(Debug, Clone)]
pub struct SurfaceState {
    pub surface_id: String,
    pub agent_id: String,
    pub view_id: String,
    pub surface_type: String,
    pub components: Option<serde_json::Value>,
    pub data_model: Option<serde_json::Value>,
}

/// Manages A2UI surface lifecycle and message routing.
///
/// Responsibilities:
/// - Track active surfaces in memory
/// - Broadcast A2UI messages to connected clients via ClientHub
/// - Persist surface state to SQLite for restoration
pub struct A2UIManager {
    hub: Arc<ClientHub>,
    store: Arc<Store>,
    catalog_provider: Arc<NeboCatalogProvider>,
    /// Active surfaces indexed by surface_id.
    surfaces: RwLock<HashMap<String, SurfaceState>>,
    /// Optional data binding manager for MCP tool polling.
    binding_manager: RwLock<Option<Arc<crate::a2ui_bindings::DataBindingManager>>>,
    /// Actions currently being processed (keyed by "surface_id:action_name").
    /// Prevents duplicate LLM dispatch and drives frontend button state.
    pending_actions: RwLock<std::collections::HashSet<String>>,
}

impl A2UIManager {
    pub fn new(
        hub: Arc<ClientHub>,
        store: Arc<Store>,
        catalog_provider: Arc<NeboCatalogProvider>,
    ) -> Self {
        Self {
            hub,
            store,
            catalog_provider,
            surfaces: RwLock::new(HashMap::new()),
            binding_manager: RwLock::new(None),
            pending_actions: RwLock::new(std::collections::HashSet::new()),
        }
    }

    /// Set the data binding manager (called after MCP bridge is available).
    pub async fn set_binding_manager(&self, manager: Arc<crate::a2ui_bindings::DataBindingManager>) {
        *self.binding_manager.write().await = Some(manager);
    }

    /// Get the catalog provider (for prompt generation and validation).
    pub fn catalog_provider(&self) -> &NeboCatalogProvider {
        &self.catalog_provider
    }

    /// Build a surface ID from agent and view IDs.
    pub fn surface_id(agent_id: &str, view_id: &str) -> String {
        format!("agent:{agent_id}:{view_id}")
    }

    /// Check if a surface with the given ID currently exists.
    pub async fn has_surface(&self, surface_id: &str) -> bool {
        self.surfaces.read().await.contains_key(surface_id)
    }

    /// Try to mark an action as pending. Returns `true` if the action was
    /// not already pending (caller should proceed). Returns `false` if
    /// it's already in-flight (caller should skip to prevent duplicate LLM dispatch).
    pub async fn try_begin_action(&self, surface_id: &str, action_name: &str) -> bool {
        let key = format!("{}:{}", surface_id, action_name);
        let inserted = self.pending_actions.write().await.insert(key);
        if inserted {
            // Notify frontend so the button can show loading state
            self.hub.broadcast(
                "a2ui_action_status",
                json!({
                    "surfaceId": surface_id,
                    "actionName": action_name,
                    "status": "processing",
                }),
            );
        }
        inserted
    }

    /// Mark an action as complete, allowing it to be triggered again.
    pub async fn end_action(&self, surface_id: &str, action_name: &str) {
        let key = format!("{}:{}", surface_id, action_name);
        self.pending_actions.write().await.remove(&key);
        // Notify frontend so the button can re-enable
        self.hub.broadcast(
            "a2ui_action_status",
            json!({
                "surfaceId": surface_id,
                "actionName": action_name,
                "status": "complete",
            }),
        );
    }

    /// Broadcast a raw A2UI message to all connected clients.
    /// The message is wrapped in an `a2ui_message` event envelope.
    pub fn broadcast_a2ui_message(&self, surface_id: &str, message: serde_json::Value) {
        info!(surface_id = %surface_id, "broadcasting a2ui_message to clients");
        self.hub.broadcast(
            "a2ui_message",
            json!({
                "surface_id": surface_id,
                "message": message,
            }),
        );
    }

    /// Create a new surface and broadcast the createSurface message.
    pub async fn create_surface(
        &self,
        agent_id: &str,
        view_id: &str,
        surface_type: &str,
        catalog_id: &str,
        theme: Option<serde_json::Value>,
    ) -> Result<String, types::NeboError> {
        let sid = Self::surface_id(agent_id, view_id);

        // Build createSurface message using a2ui-core builder
        let msg = a2ui_core::message::CreateSurfaceBuilder::new(&sid, catalog_id);
        let msg = if let Some(t) = theme {
            msg.theme(t)
        } else {
            msg
        };
        let a2ui_msg = msg.build();
        let serialized = serde_json::to_value(&a2ui_msg)
            .map_err(|e| types::NeboError::Internal(format!("a2ui serialize: {e}")))?;

        // Track in memory
        {
            let mut surfaces = self.surfaces.write().await;
            surfaces.insert(
                sid.clone(),
                SurfaceState {
                    surface_id: sid.clone(),
                    agent_id: agent_id.to_string(),
                    view_id: view_id.to_string(),
                    surface_type: surface_type.to_string(),
                    components: None,
                    data_model: None,
                },
            );
        }

        // Persist to DB
        if let Err(e) = self.store.upsert_a2ui_surface(
            &sid,
            agent_id,
            view_id,
            surface_type,
            None,
            None,
        ) {
            warn!("failed to persist a2ui surface {}: {}", sid, e);
        }

        // Broadcast to clients
        self.broadcast_a2ui_message(&sid, serialized);
        debug!("created a2ui surface: {}", sid);

        Ok(sid)
    }

    /// Normalize a component value from LLM format to A2UI wire format.
    ///
    /// LLMs may send `{"type":"heading","props":{"text":"Hi"}}` but
    /// @a2ui/web_core expects `{"component":"Text","text":"Hi","variant":"h1"}`.
    fn normalize_component(mut comp: serde_json::Value) -> serde_json::Value {
        // Handle stringified JSON (LLM sometimes produces strings instead of objects)
        if let Some(s) = comp.as_str() {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
                comp = parsed;
            } else {
                // Unrecoverable: wrap as a Text component
                return json!({
                    "id": format!("auto-{}", uuid::Uuid::new_v4().as_simple()),
                    "component": "Text",
                    "text": s,
                });
            }
        }
        if let Some(obj) = comp.as_object_mut() {
            // Auto-assign ID if missing
            if !obj.contains_key("id") {
                let id = format!("auto-{}", uuid::Uuid::new_v4().as_simple());
                obj.insert("id".to_string(), serde_json::json!(id));
            }
            // Rename "type" → "component" if "component" is missing
            if !obj.contains_key("component") {
                if let Some(t) = obj.remove("type") {
                    obj.insert("component".to_string(), t);
                }
            }
            // Flatten "props" into root level
            if let Some(props) = obj.remove("props") {
                if let Some(props_obj) = props.as_object() {
                    for (k, v) in props_obj {
                        obj.entry(k.clone()).or_insert(v.clone());
                    }
                }
            }
            // Map lowercase/alias component names to PascalCase catalog names
            if let Some(comp_type) = obj.get("component").and_then(|v| v.as_str()).map(|s| s.to_string()) {
                let mapped = match comp_type.as_str() {
                    "text" => "Text",
                    "heading" => {
                        // "heading" → Text with variant derived from "level"
                        let level = obj.remove("level")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(1);
                        let variant = match level {
                            1 => "h1", 2 => "h2", 3 => "h3",
                            4 => "h4", 5 => "h5", _ => "h1",
                        };
                        obj.entry("variant".to_string())
                            .or_insert(json!(variant));
                        // Rename "content" to "text" if present
                        if let Some(content) = obj.remove("content") {
                            obj.entry("text".to_string()).or_insert(content);
                        }
                        "Text"
                    }
                    "button" => "Button",
                    "input" => "TextField",
                    "select" | "choicePicker" | "choice_picker" => "ChoicePicker",
                    "toggle" | "checkbox" => "CheckBox",
                    "slider" => "Slider",
                    "list" => "List",
                    "tabs" => "Tabs",
                    "modal" => "Modal",
                    "icon" => "Icon",
                    "divider" => "Divider",
                    "image" => {
                        // Rename "src" to "url", "alt" to "description"
                        if let Some(src) = obj.remove("src") {
                            obj.entry("url".to_string()).or_insert(src);
                        }
                        if let Some(alt) = obj.remove("alt") {
                            obj.entry("description".to_string()).or_insert(alt);
                        }
                        "Image"
                    }
                    "column" => "Column",
                    "row" => "Row",
                    "card" => "Card",
                    _ => comp_type.as_str(),
                };
                if mapped != comp_type {
                    obj.insert("component".to_string(), json!(mapped));
                }
            }
            // For Text components: rename "content" → "text" if "text" is missing
            if obj.get("component").and_then(|v| v.as_str()) == Some("Text") {
                if !obj.contains_key("text") {
                    if let Some(content) = obj.remove("content") {
                        obj.insert("text".to_string(), content);
                    }
                }
            }
            // For Button: if "label" present but no "child", create inline text
            // (the agent should ideally send a separate Text child, but this is a fallback)
            if obj.get("component").and_then(|v| v.as_str()) == Some("Button") {
                if let Some(label) = obj.remove("label") {
                    if !obj.contains_key("child") {
                        // Store label for post-processing in ensure_root
                        obj.insert("_auto_label".to_string(), label);
                    }
                }
                // Normalize action format: LLMs send { "type": "send", "event": "click" }
                // but A2UI expects { "event": { "name": "click" } }
                if let Some(action) = obj.get_mut("action") {
                    if let Some(action_obj) = action.as_object() {
                        if !action_obj.contains_key("event") || action_obj.get("event").and_then(|e| e.as_str()).is_some() {
                            // Flat format — convert to nested
                            let event_name = action_obj.get("event")
                                .and_then(|v| v.as_str())
                                .or_else(|| action_obj.get("name").and_then(|v| v.as_str()))
                                .unwrap_or("click")
                                .to_string();
                            *action = json!({ "event": { "name": event_name } });
                        }
                    }
                }
            }
        }
        comp
    }

    /// Post-process components: auto-generate Button label children and
    /// ensure a root Column exists.
    fn ensure_root(mut components: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
        // Auto-generate Text children for Buttons with _auto_label
        let mut extra: Vec<serde_json::Value> = Vec::new();
        for comp in components.iter_mut() {
            if let Some(obj) = comp.as_object_mut() {
                if let Some(label) = obj.remove("_auto_label") {
                    let btn_id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("btn");
                    let label_id = format!("{btn_id}-label");
                    obj.insert("child".to_string(), json!(label_id));
                    extra.push(json!({
                        "id": label_id,
                        "component": "Text",
                        "text": label,
                    }));
                }
            }
        }
        components.extend(extra);

        // Ensure root Column exists
        let has_root = components.iter().any(|c| {
            c.get("id").and_then(|v| v.as_str()) == Some("root")
        });
        if !has_root && !components.is_empty() {
            let child_ids: Vec<serde_json::Value> = components
                .iter()
                .filter_map(|c| c.get("id").cloned())
                .collect();
            let root = json!({
                "id": "root",
                "component": "Column",
                "children": child_ids,
            });
            components.insert(0, root);
        }
        components
    }

    /// Push component updates to an existing surface.
    pub async fn update_components(
        &self,
        surface_id: &str,
        components: Vec<serde_json::Value>,
    ) -> Result<(), types::NeboError> {
        let components: Vec<serde_json::Value> =
            components.into_iter().map(Self::normalize_component).collect();
        let components = Self::ensure_root(components);

        let msg = a2ui_core::message::UpdateComponentsBuilder::new(surface_id)
            .components(components.clone())
            .build();
        let serialized = serde_json::to_value(&msg)
            .map_err(|e| types::NeboError::Internal(format!("a2ui serialize: {e}")))?;

        // Update in-memory state
        let components_value = serde_json::Value::Array(components);
        {
            let mut surfaces = self.surfaces.write().await;
            if let Some(state) = surfaces.get_mut(surface_id) {
                state.components = Some(components_value.clone());
            }
        }

        // Persist components (store the raw array, not the full protocol message)
        if let Ok(json_str) = serde_json::to_string(&components_value) {
            if let Err(e) = self
                .store
                .update_a2ui_surface_components(surface_id, &json_str)
            {
                warn!("failed to persist a2ui components for {}: {}", surface_id, e);
            }
        }

        self.broadcast_a2ui_message(surface_id, serialized);
        Ok(())
    }

    /// Push a data model update to an existing surface.
    pub async fn update_data_model(
        &self,
        surface_id: &str,
        path: Option<&str>,
        value: serde_json::Value,
    ) -> Result<(), types::NeboError> {
        // Unwrap string-encoded JSON: the tool schema types `value` as string,
        // so agents may pass `"[{\"name\":\"Test\"}]"` instead of an actual array.
        let value = match &value {
            serde_json::Value::String(s) => {
                serde_json::from_str::<serde_json::Value>(s).unwrap_or(value)
            }
            _ => value,
        };

        let mut builder = a2ui_core::message::UpdateDataModelBuilder::new(surface_id);
        if let Some(p) = path {
            builder = builder.path(p);
        }
        let msg = builder.value(value.clone()).build();
        let serialized = serde_json::to_value(&msg)
            .map_err(|e| types::NeboError::Internal(format!("a2ui serialize: {e}")))?;

        // Update in-memory data model — merge at path if specified
        let merged_model = {
            let mut surfaces = self.surfaces.write().await;
            if let Some(state) = surfaces.get_mut(surface_id) {
                let merged = if let Some(p) = path {
                    let mut model = state.data_model.clone().unwrap_or(json!({}));
                    set_at_pointer(&mut model, p, value.clone());
                    model
                } else {
                    value.clone()
                };
                state.data_model = Some(merged.clone());
                merged
            } else {
                value.clone()
            }
        };

        // Persist the full merged model (not just the pathed value)
        if let Ok(json_str) = serde_json::to_string(&merged_model) {
            if let Err(e) = self
                .store
                .update_a2ui_surface_data_model(surface_id, &json_str)
            {
                warn!(
                    "failed to persist a2ui data model for {}: {}",
                    surface_id, e
                );
            }
        }

        self.broadcast_a2ui_message(surface_id, serialized);
        Ok(())
    }

    /// Navigate from one view to another: delete the old surface, create the new one,
    /// and push the target view's components from views.json.
    pub async fn navigate_view(
        &self,
        agent_id: &str,
        from_view: &str,
        to_view: &str,
        params: Option<serde_json::Value>,
        views_json: &serde_json::Value,
    ) -> Result<String, types::NeboError> {
        let target = views_json.get(to_view).ok_or_else(|| {
            types::NeboError::Validation(format!("view '{}' not found in views.json", to_view))
        })?;
        let components = target
            .get("components")
            .and_then(|c| c.as_array())
            .ok_or_else(|| {
                types::NeboError::Validation(format!(
                    "view '{}' has no components array",
                    to_view
                ))
            })?;

        // Delete the source surface
        let from_sid = Self::surface_id(agent_id, from_view);
        let _ = self.delete_surface(&from_sid).await;

        // Create the target surface
        let catalog_id = "https://a2ui.org/specification/v0_9/basic_catalog.json";
        let surface_type = target
            .get("surface_type")
            .and_then(|v| v.as_str())
            .unwrap_or("panel");
        let sid = self
            .create_surface(agent_id, to_view, surface_type, catalog_id, None)
            .await?;

        // Push components
        self.update_components(&sid, components.clone()).await?;

        // Set params as initial data model if provided
        if let Some(p) = params {
            self.update_data_model(&sid, None, p).await?;
        } else if let Some(data) = target.get("data") {
            // Use view's default data if no params
            self.update_data_model(&sid, None, data.clone()).await?;
        }

        // Start data bindings for the target view
        self.start_data_bindings(&sid, target).await;

        info!(agent_id = %agent_id, from = %from_view, to = %to_view, "navigated view");
        Ok(sid)
    }

    /// Start data bindings for a surface if the view has data_bindings defined.
    pub async fn start_data_bindings(&self, surface_id: &str, view: &serde_json::Value) {
        let bindings = crate::a2ui_bindings::parse_bindings(view);
        if bindings.is_empty() {
            return;
        }
        if let Some(ref manager) = *self.binding_manager.read().await {
            manager.start_bindings(surface_id, bindings).await;
        }
    }

    /// Stop data bindings for a surface.
    pub async fn stop_data_bindings(&self, surface_id: &str) {
        if let Some(ref manager) = *self.binding_manager.read().await {
            manager.stop_bindings(surface_id).await;
        }
    }

    /// Delete a surface and broadcast the deleteSurface message.
    pub async fn delete_surface(&self, surface_id: &str) -> Result<(), types::NeboError> {
        // Stop any active data bindings before deleting
        self.stop_data_bindings(surface_id).await;

        let msg = a2ui_core::message::delete_surface_v09(surface_id);
        let serialized = serde_json::to_value(&msg)
            .map_err(|e| types::NeboError::Internal(format!("a2ui serialize: {e}")))?;

        // Remove from memory
        {
            let mut surfaces = self.surfaces.write().await;
            surfaces.remove(surface_id);
        }

        // Deactivate in DB (don't delete — preserves geometry for window restore)
        if let Err(e) = self.store.deactivate_a2ui_surface(surface_id) {
            warn!("failed to deactivate a2ui surface {}: {}", surface_id, e);
        }

        self.broadcast_a2ui_message(surface_id, serialized);
        debug!("deleted a2ui surface: {}", surface_id);
        Ok(())
    }

    /// Build A2UI protocol messages to replay a surface from stored state.
    ///
    /// Constructs raw JSON messages matching the wire format the frontend expects,
    /// bypassing the builders to avoid any field transformations.
    pub fn replay_messages_for_surface(
        state: &SurfaceState,
        catalog_id: &str,
    ) -> Vec<serde_json::Value> {
        let mut msgs = Vec::new();

        // 1. createSurface
        msgs.push(json!({
            "version": "v0.9",
            "createSurface": {
                "surfaceId": state.surface_id,
                "catalogId": catalog_id,
            }
        }));

        // 2. updateComponents (if any)
        if let Some(ref components) = state.components {
            // Handle both raw array format and legacy full-message format
            let arr = components.as_array().cloned().or_else(|| {
                components
                    .get("updateComponents")
                    .and_then(|u| u.get("components"))
                    .and_then(|c| c.as_array())
                    .cloned()
            });
            if let Some(arr) = arr {
                // Re-normalize in case components were stored in LLM format
                let normalized: Vec<serde_json::Value> = arr
                    .into_iter()
                    .map(Self::normalize_component)
                    .collect();
                let normalized = Self::ensure_root(normalized);
                msgs.push(json!({
                    "version": "v0.9",
                    "updateComponents": {
                        "surfaceId": state.surface_id,
                        "components": normalized,
                    }
                }));
            }
        }

        // 3. updateDataModel (if any)
        if let Some(ref data_model) = state.data_model {
            // Handle both raw value format and legacy full-message format
            let value = if data_model.get("updateDataModel").is_some() {
                data_model
                    .get("updateDataModel")
                    .and_then(|u| u.get("value"))
                    .cloned()
                    .unwrap_or_else(|| data_model.clone())
            } else {
                data_model.clone()
            };
            msgs.push(json!({
                "version": "v0.9",
                "updateDataModel": {
                    "surfaceId": state.surface_id,
                    "value": value,
                }
            }));
        }

        msgs
    }

    /// Get replay messages for all active surfaces of an agent.
    /// Each message is wrapped in the same `{"surface_id", "message"}` envelope
    /// that `broadcast_a2ui_message` uses, so the frontend handler can process them.
    pub async fn get_agent_replay_messages(&self, agent_id: &str) -> Vec<serde_json::Value> {
        let catalog_id = "https://a2ui.org/specification/v0_9/basic_catalog.json";
        let surfaces = self.surfaces.read().await;
        let mut messages = Vec::new();
        for state in surfaces.values().filter(|s| s.agent_id == agent_id) {
            for msg in Self::replay_messages_for_surface(state, catalog_id) {
                messages.push(json!({
                    "surface_id": state.surface_id,
                    "message": msg,
                }));
            }
        }
        messages
    }

    /// Get the current state of a surface (for context injection into agent prompts).
    pub async fn get_surface_state(&self, surface_id: &str) -> Option<SurfaceState> {
        let surfaces = self.surfaces.read().await;
        surfaces.get(surface_id).cloned()
    }

    /// List all active surfaces for an agent.
    pub async fn list_agent_surfaces(&self, agent_id: &str) -> Vec<SurfaceState> {
        let surfaces = self.surfaces.read().await;
        surfaces
            .values()
            .filter(|s| s.agent_id == agent_id)
            .cloned()
            .collect()
    }

    /// Extract the components array from a stored value, handling both
    /// raw array format and legacy full-message format.
    fn extract_components(stored: serde_json::Value) -> Option<serde_json::Value> {
        if stored.is_array() {
            Some(stored)
        } else if let Some(arr) = stored
            .get("updateComponents")
            .and_then(|u| u.get("components"))
            .and_then(|c| c.as_array())
        {
            Some(serde_json::Value::Array(arr.clone()))
        } else {
            None
        }
    }

    /// Extract the data model value from a stored value, handling both
    /// raw value format and legacy full-message format.
    fn extract_data_model(stored: serde_json::Value) -> Option<serde_json::Value> {
        if stored.get("updateDataModel").is_some() {
            stored
                .get("updateDataModel")
                .and_then(|u| u.get("value"))
                .cloned()
        } else {
            Some(stored)
        }
    }

    /// Restore previously active surfaces from DB on startup.
    pub async fn restore_surfaces(&self) {
        match self.store.list_active_a2ui_surfaces() {
            Ok(rows) => {
                let mut surfaces = self.surfaces.write().await;
                for row in rows {
                    let components = row
                        .components
                        .and_then(|c| serde_json::from_str(&c).ok())
                        .and_then(Self::extract_components);
                    let data_model = row
                        .data_model
                        .and_then(|d| serde_json::from_str(&d).ok())
                        .and_then(Self::extract_data_model);
                    surfaces.insert(
                        row.id.clone(),
                        SurfaceState {
                            surface_id: row.id,
                            agent_id: row.agent_id,
                            view_id: row.view_id,
                            surface_type: row.surface_type,
                            components,
                            data_model,
                        },
                    );
                }
                debug!("restored {} a2ui surfaces from db", surfaces.len());
            }
            Err(e) => {
                warn!("failed to restore a2ui surfaces: {}", e);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// JSON Pointer helpers
// ---------------------------------------------------------------------------

/// Set a value at a JSON Pointer path (RFC 6901), creating intermediate objects as needed.
///
/// Example: `set_at_pointer(&mut root, "/contacts/active", json!([...]))` will set
/// `root["contacts"]["active"]` to the value, creating `contacts` if it doesn't exist.
fn set_at_pointer(root: &mut serde_json::Value, pointer: &str, value: serde_json::Value) {
    let parts: Vec<&str> = pointer
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    if parts.is_empty() {
        // Empty path = replace root
        *root = value;
        return;
    }

    let mut current = root;
    for (i, key) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            // Last segment — set the value
            if let Some(obj) = current.as_object_mut() {
                obj.insert((*key).to_string(), value);
            } else {
                // Current node isn't an object — replace with one containing the key
                let mut obj = serde_json::Map::new();
                obj.insert((*key).to_string(), value);
                *current = serde_json::Value::Object(obj);
            }
            return;
        }

        // Intermediate segment — descend or create
        if !current.get(*key).is_some_and(|v| v.is_object()) {
            if let Some(obj) = current.as_object_mut() {
                obj.insert((*key).to_string(), json!({}));
            } else {
                let mut obj = serde_json::Map::new();
                obj.insert((*key).to_string(), json!({}));
                *current = serde_json::Value::Object(obj);
            }
        }
        current = current.get_mut(*key).unwrap();
    }
}

// ---------------------------------------------------------------------------
// A2UIHost trait impl — bridges tools::A2UIHost → A2UIManager
// ---------------------------------------------------------------------------

impl tools::a2ui_tool::A2UIHost for A2UIManager {
    fn create_surface(
        &self,
        agent_id: &str,
        view_id: &str,
        surface_type: &str,
        catalog_id: &str,
        theme: Option<serde_json::Value>,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
        let agent_id = agent_id.to_string();
        let view_id = view_id.to_string();
        let surface_type = surface_type.to_string();
        let catalog_id = catalog_id.to_string();
        Box::pin(async move {
            self.create_surface(&agent_id, &view_id, &surface_type, &catalog_id, theme)
                .await
                .map_err(|e| e.to_string())
        })
    }

    fn update_components(
        &self,
        surface_id: &str,
        components: Vec<serde_json::Value>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        let surface_id = surface_id.to_string();
        Box::pin(async move {
            self.update_components(&surface_id, components)
                .await
                .map_err(|e| e.to_string())
        })
    }

    fn update_data_model(
        &self,
        surface_id: &str,
        path: Option<&str>,
        value: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        let surface_id = surface_id.to_string();
        let path = path.map(|s| s.to_string());
        Box::pin(async move {
            self.update_data_model(&surface_id, path.as_deref(), value)
                .await
                .map_err(|e| e.to_string())
        })
    }

    fn delete_surface(
        &self,
        surface_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        let surface_id = surface_id.to_string();
        Box::pin(async move {
            self.delete_surface(&surface_id)
                .await
                .map_err(|e| e.to_string())
        })
    }

    fn list_surfaces(
        &self,
        agent_id: &str,
    ) -> Pin<Box<dyn Future<Output = Vec<tools::a2ui_tool::SurfaceSummary>> + Send + '_>> {
        let agent_id = agent_id.to_string();
        Box::pin(async move {
            self.list_agent_surfaces(&agent_id)
                .await
                .into_iter()
                .map(|s| tools::a2ui_tool::SurfaceSummary {
                    surface_id: s.surface_id,
                    view_id: s.view_id,
                    surface_type: s.surface_type,
                })
                .collect()
        })
    }

    fn navigate_view(
        &self,
        agent_id: &str,
        from_view: &str,
        to_view: &str,
        params: Option<serde_json::Value>,
        views_json: &serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
        let agent_id = agent_id.to_string();
        let from_view = from_view.to_string();
        let to_view = to_view.to_string();
        let views_json = views_json.clone();
        Box::pin(async move {
            self.navigate_view(&agent_id, &from_view, &to_view, params, &views_json)
                .await
                .map_err(|e| e.to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_at_pointer_creates_nested_path() {
        let mut root = json!({"contacts": {"active": [], "dormant": []}});
        set_at_pointer(&mut root, "/contacts/active", json!([{"name": "Test"}]));
        assert_eq!(root["contacts"]["active"], json!([{"name": "Test"}]));
        // Sibling path should be preserved
        assert_eq!(root["contacts"]["dormant"], json!([]));
    }

    #[test]
    fn test_set_at_pointer_sequential_updates_preserve_siblings() {
        let mut root = json!({"contacts": {"active": [], "in_window": [], "dormant": []}});
        set_at_pointer(&mut root, "/contacts/active", json!([{"name": "A"}]));
        set_at_pointer(&mut root, "/contacts/dormant", json!([{"name": "B"}]));
        assert_eq!(root["contacts"]["active"], json!([{"name": "A"}]));
        assert_eq!(root["contacts"]["dormant"], json!([{"name": "B"}]));
        assert_eq!(root["contacts"]["in_window"], json!([]));
    }

    #[test]
    fn test_set_at_pointer_creates_intermediate_objects() {
        let mut root = json!({});
        set_at_pointer(&mut root, "/a/b/c", json!("deep"));
        assert_eq!(root["a"]["b"]["c"], json!("deep"));
    }

    #[test]
    fn test_set_at_pointer_empty_path_replaces_root() {
        let mut root = json!({"old": true});
        set_at_pointer(&mut root, "", json!({"new": true}));
        assert_eq!(root, json!({"new": true}));
    }

    #[test]
    fn test_set_at_pointer_single_key() {
        let mut root = json!({"x": 1});
        set_at_pointer(&mut root, "/y", json!(2));
        assert_eq!(root, json!({"x": 1, "y": 2}));
    }
}
