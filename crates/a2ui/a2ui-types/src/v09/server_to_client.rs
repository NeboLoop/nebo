use serde::{Deserialize, Serialize};

use crate::common::SurfaceId;

/// A single server-to-client message in the v0.9 A2UI stream.
/// Each message must contain exactly one of the four message types,
/// plus a mandatory `version` field set to `"v0.9"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerToClientMessage {
    /// Protocol version — always `"v0.9"`.
    pub version: String,

    /// Creates a new surface and begins rendering.
    #[serde(rename = "createSurface", skip_serializing_if = "Option::is_none")]
    pub create_surface: Option<CreateSurface>,

    /// Updates a surface with new or modified components.
    #[serde(rename = "updateComponents", skip_serializing_if = "Option::is_none")]
    pub update_components: Option<UpdateComponents>,

    /// Updates the data model for a surface.
    #[serde(rename = "updateDataModel", skip_serializing_if = "Option::is_none")]
    pub update_data_model: Option<UpdateDataModel>,

    /// Deletes a surface.
    #[serde(rename = "deleteSurface", skip_serializing_if = "Option::is_none")]
    pub delete_surface: Option<DeleteSurface>,
}

/// Signals the client to create a new surface and begin rendering it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSurface {
    /// The unique identifier for the UI surface to be rendered.
    #[serde(rename = "surfaceId")]
    pub surface_id: SurfaceId,

    /// A string that uniquely identifies the catalog used for this surface.
    #[serde(rename = "catalogId")]
    pub catalog_id: String,

    /// Theme parameters for the surface (e.g., `{"primaryColor": "#FF0000"}`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<serde_json::Value>,

    /// If true, the client will send the full data model in every message metadata.
    #[serde(rename = "sendDataModel", skip_serializing_if = "Option::is_none")]
    pub send_data_model: Option<bool>,
}

/// Updates a surface with a new set of components.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateComponents {
    /// The unique identifier for the UI surface to be updated.
    #[serde(rename = "surfaceId")]
    pub surface_id: SurfaceId,

    /// A list of component objects (flat adjacency list).
    /// Component properties are catalog-defined, so they are represented as generic JSON.
    pub components: Vec<serde_json::Value>,
}

/// Updates the data model for a surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateDataModel {
    /// The unique identifier for the UI surface.
    #[serde(rename = "surfaceId")]
    pub surface_id: SurfaceId,

    /// A JSON Pointer to the location in the data model to update.
    /// Defaults to `/` (entire data model) if omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,

    /// The new value for the specified path. If omitted, the key at `path` is removed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
}

/// Signals the client to delete a surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteSurface {
    /// The unique identifier for the UI surface to be deleted.
    #[serde(rename = "surfaceId")]
    pub surface_id: SurfaceId,
}
