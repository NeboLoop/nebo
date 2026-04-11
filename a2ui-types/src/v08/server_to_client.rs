use serde::{Deserialize, Serialize};

use crate::common::SurfaceId;

use super::data_model::DataEntry;

/// A single server-to-client message in the v0.8 A2UI JSONL stream.
/// Each message must contain exactly one of the four message types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerToClientMessage {
    /// Signals the client to begin rendering a surface.
    #[serde(rename = "beginRendering", skip_serializing_if = "Option::is_none")]
    pub begin_rendering: Option<BeginRendering>,

    /// Updates a surface with a new set of components.
    #[serde(rename = "surfaceUpdate", skip_serializing_if = "Option::is_none")]
    pub surface_update: Option<SurfaceUpdate>,

    /// Updates the data model for a surface.
    #[serde(rename = "dataModelUpdate", skip_serializing_if = "Option::is_none")]
    pub data_model_update: Option<DataModelUpdate>,

    /// Signals the client to delete a surface.
    #[serde(rename = "deleteSurface", skip_serializing_if = "Option::is_none")]
    pub delete_surface: Option<DeleteSurface>,
}

/// Signals the client to begin rendering a surface with a root component and styles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeginRendering {
    /// The unique identifier for the UI surface to be rendered.
    #[serde(rename = "surfaceId")]
    pub surface_id: SurfaceId,

    /// The identifier of the component catalog to use for this surface.
    /// If omitted, defaults to the standard catalog for v0.8.
    #[serde(rename = "catalogId", skip_serializing_if = "Option::is_none")]
    pub catalog_id: Option<String>,

    /// The ID of the root component to render.
    pub root: String,

    /// Styling information for the UI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub styles: Option<serde_json::Value>,
}

/// Updates a surface with a new set of components.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceUpdate {
    /// The unique identifier for the UI surface to be updated.
    #[serde(rename = "surfaceId", skip_serializing_if = "Option::is_none")]
    pub surface_id: Option<SurfaceId>,

    /// A list of component instances.
    pub components: Vec<ComponentInstance>,
}

/// A single component instance in the adjacency list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentInstance {
    /// Unique identifier for this component.
    pub id: String,

    /// The relative weight within a Row or Column (CSS flex-grow).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<f64>,

    /// A wrapper object containing exactly one key (the component type name)
    /// and its properties as the value. Generic because component types are
    /// defined by the catalog, not the protocol.
    pub component: serde_json::Value,
}

/// Updates the data model for a surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataModelUpdate {
    /// The unique identifier for the UI surface this data model update applies to.
    #[serde(rename = "surfaceId")]
    pub surface_id: SurfaceId,

    /// An optional path to a location within the data model.
    /// If omitted, the update applies to the root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,

    /// An array of data entries arranged as an adjacency list.
    pub contents: Vec<DataEntry>,
}

/// Signals the client to delete a surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteSurface {
    /// The unique identifier for the UI surface to be deleted.
    #[serde(rename = "surfaceId")]
    pub surface_id: SurfaceId,
}
