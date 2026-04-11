use a2ui_types::common::CatalogId;

use crate::error::A2uiError;
use crate::traits::{CatalogInfo, CatalogProvider};

/// Result of a successful catalog negotiation.
#[derive(Debug, Clone)]
pub struct NegotiationResult {
    /// The selected catalog ID.
    pub catalog_id: CatalogId,

    /// Whether the catalog was provided inline by the client.
    pub is_inline: bool,
}

/// Negotiate the best catalog to use for a new surface.
///
/// This is a pure function implementing the A2UI catalog negotiation protocol:
/// 1. The client advertises its `supported_catalog_ids` (ordered by preference).
/// 2. The server has catalogs available via the `CatalogProvider`.
/// 3. We iterate the client's list in preference order and return the first match
///    found in the server's available catalogs.
///
/// # Arguments
/// * `client_supported_ids` — Catalog IDs the client supports, ordered by preference.
/// * `provider` — The server's catalog provider.
///
/// # Returns
/// The best matching `CatalogId`, or an error if no compatible catalog is found.
pub fn negotiate_catalog(
    client_supported_ids: &[String],
    provider: &dyn CatalogProvider,
) -> Result<NegotiationResult, A2uiError> {
    let server_catalogs: Vec<CatalogInfo> = provider.available_catalogs();
    let server_ids: Vec<&str> = server_catalogs
        .iter()
        .map(|c| c.catalog_id.as_str())
        .collect();

    for client_id in client_supported_ids {
        if server_ids.contains(&client_id.as_str()) {
            return Ok(NegotiationResult {
                catalog_id: CatalogId::new(client_id.clone()),
                is_inline: false,
            });
        }
    }

    Err(A2uiError::NegotiationFailed(format!(
        "no compatible catalog found. Client supports: {:?}, server has: {:?}",
        client_supported_ids, server_ids
    )))
}

/// Negotiate catalog with support for inline catalogs from the client.
///
/// If the client provides inline catalogs, they are checked first (since the
/// client is providing the full definition). Then falls back to
/// `negotiate_catalog` for pre-defined catalogs.
///
/// # Arguments
/// * `client_supported_ids` — Catalog IDs the client supports, ordered by preference.
/// * `client_inline_catalog_ids` — Catalog IDs of inline catalogs provided by the client.
/// * `server_accepts_inline` — Whether the server accepts inline catalogs.
/// * `provider` — The server's catalog provider.
pub fn negotiate_catalog_with_inline(
    client_supported_ids: &[String],
    client_inline_catalog_ids: &[String],
    server_accepts_inline: bool,
    provider: &dyn CatalogProvider,
) -> Result<NegotiationResult, A2uiError> {
    // First, try inline catalogs if the server accepts them
    if server_accepts_inline && !client_inline_catalog_ids.is_empty() {
        // Inline catalogs are client-provided, so any of them are valid
        // Pick the first one that appears in the client's supported list
        for client_id in client_supported_ids {
            if client_inline_catalog_ids.contains(client_id) {
                return Ok(NegotiationResult {
                    catalog_id: CatalogId::new(client_id.clone()),
                    is_inline: true,
                });
            }
        }

        // If inline catalog IDs aren't in the supported list, just take the first inline
        if let Some(first_inline) = client_inline_catalog_ids.first() {
            return Ok(NegotiationResult {
                catalog_id: CatalogId::new(first_inline.clone()),
                is_inline: true,
            });
        }
    }

    // Fall back to pre-defined catalog negotiation
    negotiate_catalog(client_supported_ids, provider)
}

#[cfg(test)]
mod tests {
    use super::*;
    use a2ui_types::v09::catalog::Catalog;

    struct MockProvider {
        catalogs: Vec<CatalogInfo>,
    }

    impl CatalogProvider for MockProvider {
        fn available_catalogs(&self) -> Vec<CatalogInfo> {
            self.catalogs.clone()
        }

        fn get_catalog(&self, _id: &CatalogId) -> Option<Catalog> {
            None
        }

        fn get_catalog_schema(&self, _id: &CatalogId) -> Option<serde_json::Value> {
            None
        }
    }

    #[test]
    fn test_negotiate_first_match() {
        let provider = MockProvider {
            catalogs: vec![
                CatalogInfo {
                    catalog_id: CatalogId::new("https://example.com/basic.json"),
                    description: None,
                },
                CatalogInfo {
                    catalog_id: CatalogId::new("https://example.com/custom.json"),
                    description: None,
                },
            ],
        };

        let client_ids = vec![
            "https://example.com/custom.json".to_string(),
            "https://example.com/basic.json".to_string(),
        ];

        let result = negotiate_catalog(&client_ids, &provider).unwrap();
        assert_eq!(
            result.catalog_id.as_str(),
            "https://example.com/custom.json"
        );
        assert!(!result.is_inline);
    }

    #[test]
    fn test_negotiate_no_match() {
        let provider = MockProvider {
            catalogs: vec![CatalogInfo {
                catalog_id: CatalogId::new("https://example.com/basic.json"),
                description: None,
            }],
        };

        let client_ids = vec!["https://other.com/unknown.json".to_string()];

        let result = negotiate_catalog(&client_ids, &provider);
        assert!(result.is_err());
    }

    #[test]
    fn test_negotiate_with_inline() {
        let provider = MockProvider {
            catalogs: vec![CatalogInfo {
                catalog_id: CatalogId::new("https://example.com/basic.json"),
                description: None,
            }],
        };

        let client_ids = vec![
            "https://example.com/inline-dev.json".to_string(),
            "https://example.com/basic.json".to_string(),
        ];
        let inline_ids = vec!["https://example.com/inline-dev.json".to_string()];

        let result =
            negotiate_catalog_with_inline(&client_ids, &inline_ids, true, &provider).unwrap();
        assert_eq!(
            result.catalog_id.as_str(),
            "https://example.com/inline-dev.json"
        );
        assert!(result.is_inline);
    }

    #[test]
    fn test_negotiate_inline_not_accepted() {
        let provider = MockProvider {
            catalogs: vec![CatalogInfo {
                catalog_id: CatalogId::new("https://example.com/basic.json"),
                description: None,
            }],
        };

        let client_ids = vec![
            "https://example.com/inline-dev.json".to_string(),
            "https://example.com/basic.json".to_string(),
        ];
        let inline_ids = vec!["https://example.com/inline-dev.json".to_string()];

        // Server doesn't accept inline
        let result =
            negotiate_catalog_with_inline(&client_ids, &inline_ids, false, &provider).unwrap();
        assert_eq!(result.catalog_id.as_str(), "https://example.com/basic.json");
        assert!(!result.is_inline);
    }
}
