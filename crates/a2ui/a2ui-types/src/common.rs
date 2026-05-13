use serde::{Deserialize, Serialize};

/// Unique identifier for a UI surface.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SurfaceId(pub String);

impl SurfaceId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for SurfaceId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl From<String> for SurfaceId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Unique identifier for a component within a surface.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ComponentId(pub String);

impl ComponentId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for ComponentId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl From<String> for ComponentId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Unique identifier for a component catalog.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CatalogId(pub String);

impl CatalogId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for CatalogId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl From<String> for CatalogId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// The A2UI spec version this library supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpecVersion {
    V0_8,
    V0_9,
}

impl std::fmt::Display for SpecVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpecVersion::V0_8 => write!(f, "v0.8"),
            SpecVersion::V0_9 => write!(f, "v0.9"),
        }
    }
}
