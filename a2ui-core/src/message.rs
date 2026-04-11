use a2ui_types::common::SurfaceId;
use a2ui_types::v08::data_model::DataEntry;
use a2ui_types::v08::server_to_client as v08;
use a2ui_types::v09::server_to_client as v09;

// ---------------------------------------------------------------------------
// v0.9 Message Builders
// ---------------------------------------------------------------------------

/// Builder for a v0.9 `createSurface` message.
pub struct CreateSurfaceBuilder {
    surface_id: String,
    catalog_id: String,
    theme: Option<serde_json::Value>,
    send_data_model: Option<bool>,
}

impl CreateSurfaceBuilder {
    pub fn new(surface_id: impl Into<String>, catalog_id: impl Into<String>) -> Self {
        Self {
            surface_id: surface_id.into(),
            catalog_id: catalog_id.into(),
            theme: None,
            send_data_model: None,
        }
    }

    pub fn theme(mut self, theme: serde_json::Value) -> Self {
        self.theme = Some(theme);
        self
    }

    pub fn send_data_model(mut self, send: bool) -> Self {
        self.send_data_model = Some(send);
        self
    }

    pub fn build(self) -> v09::ServerToClientMessage {
        v09::ServerToClientMessage {
            version: "v0.9".to_string(),
            create_surface: Some(v09::CreateSurface {
                surface_id: SurfaceId::new(self.surface_id),
                catalog_id: self.catalog_id,
                theme: self.theme,
                send_data_model: self.send_data_model,
            }),
            update_components: None,
            update_data_model: None,
            delete_surface: None,
        }
    }
}

/// Builder for a v0.9 `updateComponents` message.
pub struct UpdateComponentsBuilder {
    surface_id: String,
    components: Vec<serde_json::Value>,
}

impl UpdateComponentsBuilder {
    pub fn new(surface_id: impl Into<String>) -> Self {
        Self {
            surface_id: surface_id.into(),
            components: Vec::new(),
        }
    }

    pub fn add_component(mut self, component: serde_json::Value) -> Self {
        self.components.push(component);
        self
    }

    pub fn components(mut self, components: Vec<serde_json::Value>) -> Self {
        self.components = components;
        self
    }

    pub fn build(self) -> v09::ServerToClientMessage {
        v09::ServerToClientMessage {
            version: "v0.9".to_string(),
            create_surface: None,
            update_components: Some(v09::UpdateComponents {
                surface_id: SurfaceId::new(self.surface_id),
                components: self.components,
            }),
            update_data_model: None,
            delete_surface: None,
        }
    }
}

/// Builder for a v0.9 `updateDataModel` message.
pub struct UpdateDataModelBuilder {
    surface_id: String,
    path: Option<String>,
    value: Option<serde_json::Value>,
}

impl UpdateDataModelBuilder {
    pub fn new(surface_id: impl Into<String>) -> Self {
        Self {
            surface_id: surface_id.into(),
            path: None,
            value: None,
        }
    }

    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn value(mut self, value: serde_json::Value) -> Self {
        self.value = Some(value);
        self
    }

    pub fn build(self) -> v09::ServerToClientMessage {
        v09::ServerToClientMessage {
            version: "v0.9".to_string(),
            create_surface: None,
            update_components: None,
            update_data_model: Some(v09::UpdateDataModel {
                surface_id: SurfaceId::new(self.surface_id),
                path: self.path,
                value: self.value,
            }),
            delete_surface: None,
        }
    }
}

/// Build a v0.9 `deleteSurface` message.
pub fn delete_surface_v09(surface_id: impl Into<String>) -> v09::ServerToClientMessage {
    v09::ServerToClientMessage {
        version: "v0.9".to_string(),
        create_surface: None,
        update_components: None,
        update_data_model: None,
        delete_surface: Some(v09::DeleteSurface {
            surface_id: SurfaceId::new(surface_id),
        }),
    }
}

// ---------------------------------------------------------------------------
// v0.8 Message Builders
// ---------------------------------------------------------------------------

/// Builder for a v0.8 `beginRendering` message.
pub struct BeginRenderingBuilder {
    surface_id: String,
    root: String,
    catalog_id: Option<String>,
    styles: Option<serde_json::Value>,
}

impl BeginRenderingBuilder {
    pub fn new(surface_id: impl Into<String>, root: impl Into<String>) -> Self {
        Self {
            surface_id: surface_id.into(),
            root: root.into(),
            catalog_id: None,
            styles: None,
        }
    }

    pub fn catalog_id(mut self, catalog_id: impl Into<String>) -> Self {
        self.catalog_id = Some(catalog_id.into());
        self
    }

    pub fn styles(mut self, styles: serde_json::Value) -> Self {
        self.styles = Some(styles);
        self
    }

    pub fn build(self) -> v08::ServerToClientMessage {
        v08::ServerToClientMessage {
            begin_rendering: Some(v08::BeginRendering {
                surface_id: SurfaceId::new(self.surface_id),
                catalog_id: self.catalog_id,
                root: self.root,
                styles: self.styles,
            }),
            surface_update: None,
            data_model_update: None,
            delete_surface: None,
        }
    }
}

/// Builder for a v0.8 `surfaceUpdate` message.
pub struct SurfaceUpdateBuilder {
    surface_id: Option<String>,
    components: Vec<v08::ComponentInstance>,
}

impl SurfaceUpdateBuilder {
    pub fn new() -> Self {
        Self {
            surface_id: None,
            components: Vec::new(),
        }
    }

    pub fn surface_id(mut self, surface_id: impl Into<String>) -> Self {
        self.surface_id = Some(surface_id.into());
        self
    }

    pub fn add_component(mut self, component: v08::ComponentInstance) -> Self {
        self.components.push(component);
        self
    }

    pub fn components(mut self, components: Vec<v08::ComponentInstance>) -> Self {
        self.components = components;
        self
    }

    pub fn build(self) -> v08::ServerToClientMessage {
        v08::ServerToClientMessage {
            begin_rendering: None,
            surface_update: Some(v08::SurfaceUpdate {
                surface_id: self.surface_id.map(SurfaceId::new),
                components: self.components,
            }),
            data_model_update: None,
            delete_surface: None,
        }
    }
}

impl Default for SurfaceUpdateBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for a v0.8 `dataModelUpdate` message.
pub struct DataModelUpdateBuilder {
    surface_id: String,
    path: Option<String>,
    contents: Vec<DataEntry>,
}

impl DataModelUpdateBuilder {
    pub fn new(surface_id: impl Into<String>) -> Self {
        Self {
            surface_id: surface_id.into(),
            path: None,
            contents: Vec::new(),
        }
    }

    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn add_entry(mut self, entry: DataEntry) -> Self {
        self.contents.push(entry);
        self
    }

    pub fn contents(mut self, contents: Vec<DataEntry>) -> Self {
        self.contents = contents;
        self
    }

    pub fn build(self) -> v08::ServerToClientMessage {
        v08::ServerToClientMessage {
            begin_rendering: None,
            surface_update: None,
            data_model_update: Some(v08::DataModelUpdate {
                surface_id: SurfaceId::new(self.surface_id),
                path: self.path,
                contents: self.contents,
            }),
            delete_surface: None,
        }
    }
}

/// Build a v0.8 `deleteSurface` message.
pub fn delete_surface_v08(surface_id: impl Into<String>) -> v08::ServerToClientMessage {
    v08::ServerToClientMessage {
        begin_rendering: None,
        surface_update: None,
        data_model_update: None,
        delete_surface: Some(v08::DeleteSurface {
            surface_id: SurfaceId::new(surface_id),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_create_surface_builder() {
        let msg = CreateSurfaceBuilder::new("my-surface", "https://example.com/catalog.json")
            .theme(json!({"primaryColor": "#FF0000"}))
            .send_data_model(true)
            .build();

        assert_eq!(msg.version, "v0.9");
        let cs = msg.create_surface.unwrap();
        assert_eq!(cs.surface_id.as_str(), "my-surface");
        assert_eq!(cs.catalog_id, "https://example.com/catalog.json");
        assert!(cs.theme.is_some());
        assert_eq!(cs.send_data_model, Some(true));
    }

    #[test]
    fn test_update_components_builder() {
        let msg = UpdateComponentsBuilder::new("s1")
            .add_component(json!({"id": "root", "component": "Column", "children": ["t1"]}))
            .add_component(json!({"id": "t1", "component": "Text", "text": "Hello"}))
            .build();

        let uc = msg.update_components.unwrap();
        assert_eq!(uc.components.len(), 2);
    }

    #[test]
    fn test_update_data_model_builder() {
        let msg = UpdateDataModelBuilder::new("s1")
            .path("/user/name")
            .value(json!("Alice"))
            .build();

        let udm = msg.update_data_model.unwrap();
        assert_eq!(udm.path.unwrap(), "/user/name");
        assert_eq!(udm.value.unwrap(), json!("Alice"));
    }

    #[test]
    fn test_delete_surface_v09() {
        let msg = delete_surface_v09("s1");
        let ds = msg.delete_surface.unwrap();
        assert_eq!(ds.surface_id.as_str(), "s1");
    }

    #[test]
    fn test_begin_rendering_builder() {
        let msg = BeginRenderingBuilder::new("main", "root")
            .catalog_id("https://example.com/catalog.json")
            .build();

        let br = msg.begin_rendering.unwrap();
        assert_eq!(br.surface_id.as_str(), "main");
        assert_eq!(br.root, "root");
        assert_eq!(br.catalog_id.unwrap(), "https://example.com/catalog.json");
    }

    #[test]
    fn test_surface_update_builder() {
        let msg = SurfaceUpdateBuilder::new()
            .surface_id("main")
            .add_component(v08::ComponentInstance {
                id: "root".to_string(),
                weight: None,
                component: json!({"Column": {"children": {"explicitList": ["t1"]}}}),
            })
            .build();

        let su = msg.surface_update.unwrap();
        assert_eq!(su.surface_id.unwrap().as_str(), "main");
        assert_eq!(su.components.len(), 1);
    }

    #[test]
    fn test_data_model_update_builder() {
        let msg = DataModelUpdateBuilder::new("main")
            .path("user")
            .add_entry(DataEntry {
                key: "name".to_string(),
                value_string: Some("Bob".to_string()),
                value_number: None,
                value_boolean: None,
                value_map: None,
            })
            .build();

        let dmu = msg.data_model_update.unwrap();
        assert_eq!(dmu.surface_id.as_str(), "main");
        assert_eq!(dmu.contents.len(), 1);
        assert_eq!(dmu.contents[0].value_string.as_ref().unwrap(), "Bob");
    }
}
