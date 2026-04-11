use thiserror::Error;

/// Top-level error type for the a2ui-core library.
#[derive(Debug, Error)]
pub enum A2uiError {
    /// Serialization or deserialization failed.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Message validation failed against the catalog schema.
    #[error("validation failed with {} error(s)", .0.len())]
    Validation(Vec<a2ui_validation::ValidationError>),

    /// Catalog negotiation failed — no compatible catalog found.
    #[error("catalog negotiation failed: {0}")]
    NegotiationFailed(String),

    /// The requested catalog was not found.
    #[error("catalog not found: {0}")]
    CatalogNotFound(String),

    /// Transport error when sending a message to the client.
    #[error("transport error: {0}")]
    Transport(String),

    /// A generic error for unexpected conditions.
    #[error("{0}")]
    Other(String),
}
