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
use tracing::{debug, warn};

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
        let basic_id = "basic";
        let basic_catalog = Self::build_basic_catalog();
        let basic_schema = Self::build_basic_schema();

        let mut catalogs = HashMap::new();
        catalogs.insert(basic_id.to_string(), (basic_catalog, basic_schema));

        Self { catalogs }
    }

    /// Build the basic A2UI catalog with standard component types.
    fn build_basic_catalog() -> Catalog {
        serde_json::from_value(json!({
            "catalogId": "basic",
            "components": {
                "text": {
                    "type": "object",
                    "properties": {
                        "content": { "type": "string" }
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
                        "action": { "type": "object" }
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
                "toggle": {
                    "type": "object",
                    "properties": {
                        "label": { "type": "string" },
                        "value": { "type": "boolean" }
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
                "text": { "description": "Display text content" },
                "heading": { "description": "Section heading (h1-h6)" },
                "button": { "description": "Clickable button with action" },
                "input": { "description": "Text input field" },
                "select": { "description": "Dropdown selector" },
                "toggle": { "description": "Boolean toggle switch" },
                "divider": { "description": "Visual separator" },
                "image": { "description": "Image display" }
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
        }
    }

    /// Get the catalog provider (for prompt generation and validation).
    pub fn catalog_provider(&self) -> &NeboCatalogProvider {
        &self.catalog_provider
    }

    /// Build a surface ID from agent and view IDs.
    pub fn surface_id(agent_id: &str, view_id: &str) -> String {
        format!("agent:{agent_id}:{view_id}")
    }

    /// Broadcast a raw A2UI message to all connected clients.
    /// The message is wrapped in an `a2ui_message` event envelope.
    pub fn broadcast_a2ui_message(&self, surface_id: &str, message: serde_json::Value) {
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

    /// Push component updates to an existing surface.
    pub async fn update_components(
        &self,
        surface_id: &str,
        components: Vec<serde_json::Value>,
    ) -> Result<(), types::NeboError> {
        let msg = a2ui_core::message::UpdateComponentsBuilder::new(surface_id)
            .components(components.clone())
            .build();
        let serialized = serde_json::to_value(&msg)
            .map_err(|e| types::NeboError::Internal(format!("a2ui serialize: {e}")))?;

        // Update in-memory state
        {
            let mut surfaces = self.surfaces.write().await;
            if let Some(state) = surfaces.get_mut(surface_id) {
                state.components = Some(serde_json::Value::Array(components));
            }
        }

        // Persist components
        if let Ok(json_str) = serde_json::to_string(&serialized) {
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
        let mut builder = a2ui_core::message::UpdateDataModelBuilder::new(surface_id);
        if let Some(p) = path {
            builder = builder.path(p);
        }
        let msg = builder.value(value.clone()).build();
        let serialized = serde_json::to_value(&msg)
            .map_err(|e| types::NeboError::Internal(format!("a2ui serialize: {e}")))?;

        // Update in-memory data model
        {
            let mut surfaces = self.surfaces.write().await;
            if let Some(state) = surfaces.get_mut(surface_id) {
                state.data_model = Some(value);
            }
        }

        // Persist data model
        if let Ok(json_str) = serde_json::to_string(&serialized) {
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

    /// Delete a surface and broadcast the deleteSurface message.
    pub async fn delete_surface(&self, surface_id: &str) -> Result<(), types::NeboError> {
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

    /// Restore previously active surfaces from DB on startup.
    pub async fn restore_surfaces(&self) {
        match self.store.list_active_a2ui_surfaces() {
            Ok(rows) => {
                let mut surfaces = self.surfaces.write().await;
                for row in rows {
                    surfaces.insert(
                        row.id.clone(),
                        SurfaceState {
                            surface_id: row.id,
                            agent_id: row.agent_id,
                            view_id: row.view_id,
                            surface_type: row.surface_type,
                            components: row
                                .components
                                .and_then(|c| serde_json::from_str(&c).ok()),
                            data_model: row
                                .data_model
                                .and_then(|d| serde_json::from_str(&d).ok()),
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
}
