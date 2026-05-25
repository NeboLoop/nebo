use std::sync::Arc;

use serde::{Deserialize, Serialize};
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// A tool definition discovered from a sidecar's `GET /_tools` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarToolDef {
    /// Action name the LLM will use (e.g. "list_projects", "create_document").
    pub name: String,
    /// Human-readable description of what this action does.
    pub description: String,
    /// HTTP method to use when proxying (GET, POST, PUT, DELETE).
    pub method: String,
    /// Path relative to the sidecar root (e.g. "/projects", "/documents/{id}").
    pub path: String,
    /// JSON Schema for the request body (optional — omit for GET/DELETE).
    #[serde(default)]
    pub input_schema: Option<serde_json::Value>,
}

/// HTTP-like response from a sidecar call.
pub struct SidecarResponse {
    pub status_code: i32,
    pub body: Vec<u8>,
}

/// Trait for calling a sidecar's HandleRequest. Implemented in `crates/server`
/// using the gRPC connection; keeps `crates/tools` free of tonic/proto deps.
pub trait SidecarCaller: Send + Sync {
    fn call(
        &self,
        method: &str,
        path: &str,
        query: &str,
        body: &[u8],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<SidecarResponse, String>> + Send + '_>,
    >;
}

/// Individual tool that routes a single LLM tool call to a sidecar HTTP endpoint.
///
/// Each sidecar endpoint becomes its own native tool — the LLM sees
/// `list_projects(...)` directly, not `brief(action: "list_projects")`.
pub struct SidecarActionTool {
    def: SidecarToolDef,
    caller: Arc<dyn SidecarCaller>,
}

impl SidecarActionTool {
    pub fn new(def: SidecarToolDef, caller: Arc<dyn SidecarCaller>) -> Self {
        Self { def, caller }
    }

    /// Resolve path parameters like `/documents/{id}` using input values.
    fn resolve_path(template: &str, input: &serde_json::Value) -> String {
        let mut path = template.to_string();
        if let Some(obj) = input.as_object() {
            for (key, val) in obj {
                let placeholder = format!("{{{}}}", key);
                if path.contains(&placeholder) {
                    let replacement = val
                        .as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| val.to_string().trim_matches('"').to_string());
                    path = path.replace(&placeholder, &replacement);
                }
            }
        }
        path
    }
}

impl DynTool for SidecarActionTool {
    fn name(&self) -> &str {
        &self.def.name
    }

    fn description(&self) -> String {
        self.def.description.clone()
    }

    fn schema(&self) -> serde_json::Value {
        self.def.input_schema.clone().unwrap_or_else(|| {
            serde_json::json!({
                "type": "object",
                "properties": {}
            })
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
            let path = Self::resolve_path(&self.def.path, &input);
            let method = self.def.method.to_uppercase();

            let body = if method == "GET" || method == "DELETE" || method == "HEAD" {
                Vec::new()
            } else {
                let mut body_obj = input.clone();
                if let Some(obj) = body_obj.as_object_mut() {
                    // Remove keys used as path parameters
                    for segment in self.def.path.split('/') {
                        if let Some(key) =
                            segment.strip_prefix('{').and_then(|s| s.strip_suffix('}'))
                        {
                            obj.remove(key);
                        }
                    }
                }
                serde_json::to_vec(&body_obj).unwrap_or_default()
            };

            let query = if method == "GET" {
                let mut params = Vec::new();
                if let Some(obj) = input.as_object() {
                    for (key, val) in obj {
                        let placeholder = format!("{{{}}}", key);
                        if self.def.path.contains(&placeholder) {
                            continue;
                        }
                        if let Some(s) = val.as_str() {
                            params.push(format!("{}={}", key, s));
                        } else if !val.is_null() {
                            params.push(format!("{}={}", key, val));
                        }
                    }
                }
                params.join("&")
            } else {
                String::new()
            };

            match self.caller.call(&method, &path, &query, &body).await {
                Ok(resp) => {
                    let content = String::from_utf8_lossy(&resp.body).to_string();
                    if resp.status_code >= 400 {
                        ToolResult::error(format!(
                            "Sidecar returned HTTP {}: {}",
                            resp.status_code, content
                        ))
                    } else {
                        ToolResult::ok(content)
                    }
                }
                Err(e) => ToolResult::error(format!("Failed to call sidecar: {}", e)),
            }
        })
    }
}
