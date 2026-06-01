//! ToolSearch — meta-tool for deferred tool discovery.
//!
//! Lets the LLM explicitly search for and activate deferred tools by name
//! or keyword, rather than relying solely on keyword auto-activation.

use std::sync::Arc;

use serde_json::json;
use tracing::debug;

use crate::origin::ToolContext;
use crate::registry::{DynTool, Registry, ToolResult};

/// Meta-tool that searches deferred tool stubs and returns matches.
/// The runner intercepts results to activate matched tools for subsequent turns.
pub struct ToolSearchTool {
    registry: Arc<Registry>,
}

impl ToolSearchTool {
    pub fn new(registry: Arc<Registry>) -> Self {
        Self { registry }
    }
}

impl DynTool for ToolSearchTool {
    fn name(&self) -> &str {
        "tool_search"
    }

    fn description(&self) -> String {
        "Search for and activate additional tools that aren't loaded by default. \
         Use this when the user's request requires a tool not in your active set \
         (e.g., plugins, MCP integrations, workflow engine, script execution)."
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query. Modes:\n\
                        - select:name — activate a specific tool by exact name (comma-separated for multiple)\n\
                        - keywords — search by capability (e.g., \"gmail\", \"workflow\")\n\
                        - +required keyword — require match in name, rank by remaining keywords"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum results to return (default 5)",
                    "default": 5
                }
            },
            "required": ["query"]
        })
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let query = match input.get("query").and_then(|v| v.as_str()) {
                Some(q) => q.trim(),
                None => return ToolResult::error(crate::errors::missing_param(
                    "search",
                    "query",
                    "tool_search(query: \"file operations\")",
                )),
            };
            if query.is_empty() {
                return ToolResult::error(crate::errors::missing_param(
                    "search",
                    "query",
                    "tool_search(query: \"file operations\")",
                ));
            }

            let max_results = input
                .get("max_results")
                .and_then(|v| v.as_u64())
                .unwrap_or(5) as usize;

            // Get ALL deferred stubs (pass empty set to see everything)
            let empty = std::collections::HashSet::new();
            let stubs = self.registry.list_deferred_stubs(&empty).await;

            if stubs.is_empty() {
                return ToolResult::ok(
                    json!({
                        "matches": [],
                        "descriptions": {},
                        "total_deferred": 0,
                        "message": "No deferred tools are registered."
                    })
                    .to_string(),
                );
            }

            let matches = if let Some(names) = query.strip_prefix("select:") {
                // Exact name match mode
                let requested: Vec<&str> = names.split(',').map(|s| s.trim()).collect();
                stubs
                    .iter()
                    .filter(|(name, _)| requested.iter().any(|r| r.eq_ignore_ascii_case(name)))
                    .map(|(name, desc)| (name.clone(), desc.clone()))
                    .collect::<Vec<_>>()
            } else if let Some(rest) = query.strip_prefix('+') {
                // Required prefix mode: first token must match in name
                let tokens: Vec<&str> = rest.split_whitespace().collect();
                let (required, ranking) = if tokens.is_empty() {
                    ("", vec![])
                } else {
                    (tokens[0], tokens[1..].to_vec())
                };
                let required_lower = required.to_lowercase();
                let mut scored: Vec<(i32, String, String)> = stubs
                    .iter()
                    .filter(|(name, _)| name.to_lowercase().contains(&required_lower))
                    .map(|(name, desc)| {
                        let score = score_keywords(&ranking, name, desc);
                        (score, name.clone(), desc.clone())
                    })
                    .collect();
                scored.sort_by(|a, b| b.0.cmp(&a.0));
                scored
                    .into_iter()
                    .take(max_results)
                    .map(|(_, name, desc)| (name, desc))
                    .collect()
            } else {
                // Keyword search mode
                let keywords: Vec<&str> = query.split_whitespace().collect();
                let mut scored: Vec<(i32, String, String)> = stubs
                    .iter()
                    .filter_map(|(name, desc)| {
                        let score = score_keywords(&keywords, name, desc);
                        if score > 0 { Some((score, name.clone(), desc.clone())) } else { None }
                    })
                    .collect();
                scored.sort_by(|a, b| b.0.cmp(&a.0));
                scored
                    .into_iter()
                    .take(max_results)
                    .map(|(_, name, desc)| (name, desc))
                    .collect()
            };

            let match_names: Vec<&str> = matches.iter().map(|(n, _)| n.as_str()).collect();
            let descriptions: serde_json::Map<String, serde_json::Value> = matches
                .iter()
                .map(|(n, d)| (n.clone(), serde_json::Value::String(d.clone())))
                .collect();

            debug!(query, found = matches.len(), total = stubs.len(), "tool_search");

            let mut result = json!({
                "matches": match_names,
                "descriptions": descriptions,
                "total_deferred": stubs.len(),
            });
            if match_names.is_empty() {
                result["hint"] = json!("No matching tools found. Try skill(action: \"discover\", query: \"...\") to check if a skill is available for this task.");
            }
            ToolResult::ok(result.to_string())
        })
    }
}

/// Score a tool stub against keyword tokens.
/// Name matches score higher (+10 each) than description matches (+2 each).
fn score_keywords(keywords: &[&str], name: &str, desc: &str) -> i32 {
    let name_lower = name.to_lowercase();
    let desc_lower = desc.to_lowercase();
    let mut score = 0i32;
    for kw in keywords {
        let kw_lower = kw.to_lowercase();
        if name_lower.contains(&kw_lower) {
            score += 10;
        }
        if desc_lower.contains(&kw_lower) {
            score += 2;
        }
    }
    score
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_score_keywords_name_match() {
        assert_eq!(score_keywords(&["gmail"], "gws-gmail-read", "Read Gmail messages"), 12);
    }

    #[test]
    fn test_score_keywords_desc_only() {
        assert_eq!(score_keywords(&["email"], "plugin", "Send and receive email"), 2);
    }

    #[test]
    fn test_score_keywords_no_match() {
        assert_eq!(score_keywords(&["calendar"], "plugin", "Execute scripts"), 0);
    }
}
