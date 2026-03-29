use std::sync::Arc;

use tracing::warn;

use db::Store;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// PublisherTool manages publishing skills and agents to NeboLoop marketplace.
pub struct PublisherTool {
    store: Arc<Store>,
}

impl PublisherTool {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    async fn handle_publish(&self, input: &serde_json::Value) -> ToolResult {
        let artifact_type = input["type"].as_str().unwrap_or("");
        let name = input["name"].as_str().unwrap_or("");
        let visibility = input["visibility"].as_str().unwrap_or("private");
        let version = input["version"].as_str().unwrap_or("1.0.0");

        if name.is_empty() {
            return ToolResult::error("'name' is required");
        }

        let api = match crate::build_neboloop_api(&self.store) {
            Ok(a) => a,
            Err(e) => return ToolResult::error(format!("NeboLoop connection required: {}", e)),
        };

        match artifact_type {
            "agent" => self.publish_agent(&api, name, version, visibility).await,
            "skill" => self.publish_skill(&api, name, version, visibility).await,
            _ => ToolResult::error("'type' must be 'agent' or 'skill'"),
        }
    }

    async fn publish_agent(&self, api: &comm::api::NeboLoopApi, name: &str, version: &str, visibility: &str) -> ToolResult {
        let db_role = match self.store.list_agents(500, 0) {
            Ok(agents) => {
                let lower = name.to_lowercase();
                agents.into_iter().find(|r| r.name.to_lowercase() == lower || r.id == name)
            }
            Err(e) => return ToolResult::error(format!("Failed to query agents: {}", e)),
        };
        let db_role = match db_role {
            Some(r) => r,
            None => return ToolResult::error(format!("Agent '{}' not found locally.", name)),
        };

        let agent_json = if db_role.frontmatter.is_empty() || db_role.frontmatter == "{}" {
            None
        } else {
            Some(db_role.frontmatter.as_str())
        };

        match api.publish_agent(&db_role.name, &db_role.description, &db_role.agent_md, version, visibility, agent_json).await {
            Ok(result) => {
                let artifact_id = result["id"].as_str().unwrap_or("unknown");
                self.maybe_submit(api, artifact_id, version, visibility, &db_role.name, "agent").await
            }
            Err(e) => ToolResult::error(format!("Publish failed: {}", e)),
        }
    }

    async fn publish_skill(&self, api: &comm::api::NeboLoopApi, name: &str, version: &str, visibility: &str) -> ToolResult {
        // Read SKILL.md from filesystem
        let data_dir = config::data_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

        // Check user skills first, then installed
        let skill_md_path = [
            data_dir.join("user").join("skills").join(name).join("SKILL.md"),
            data_dir.join("nebo").join("skills").join(name).join("SKILL.md"),
        ];

        let (manifest_content, description) = {
            let mut found = None;
            for path in &skill_md_path {
                if path.exists() {
                    if let Ok(content) = std::fs::read_to_string(path) {
                        // Extract description from first paragraph
                        let desc = content.lines()
                            .skip_while(|l| l.starts_with('#') || l.trim().is_empty() || l.starts_with("---"))
                            .take_while(|l| !l.trim().is_empty())
                            .collect::<Vec<_>>()
                            .join(" ");
                        let desc = if desc.len() > 200 { format!("{}...", crate::truncate_str(&desc, 200)) } else { desc };
                        found = Some((content, desc));
                        break;
                    }
                }
            }
            match found {
                Some(f) => f,
                None => return ToolResult::error(format!("Skill '{}' not found. Check that SKILL.md exists in the skills directory.", name)),
            }
        };

        match api.publish_skill(name, &description, &manifest_content, version, visibility).await {
            Ok(result) => {
                let artifact_id = result["id"].as_str().unwrap_or("unknown");
                self.maybe_submit(api, artifact_id, version, visibility, name, "skill").await
            }
            Err(e) => ToolResult::error(format!("Publish failed: {}", e)),
        }
    }

    async fn maybe_submit(&self, api: &comm::api::NeboLoopApi, artifact_id: &str, version: &str, visibility: &str, name: &str, artifact_type: &str) -> ToolResult {
        if visibility == "public" {
            match api.submit_for_review(artifact_id, version).await {
                Ok(_) => ToolResult::ok(format!(
                    "Published **{}** ({}) v{} to NeboLoop and submitted for marketplace review.\nArtifact ID: {}",
                    name, artifact_type, version, artifact_id
                )),
                Err(e) => ToolResult::ok(format!(
                    "Published **{}** ({}) v{} to NeboLoop (artifact: {}) but review submission failed: {}",
                    name, artifact_type, version, artifact_id, e
                )),
            }
        } else {
            ToolResult::ok(format!(
                "Published **{}** ({}) v{} to NeboLoop as {}.\nArtifact ID: {}",
                name, artifact_type, version, visibility, artifact_id
            ))
        }
    }

    async fn handle_list(&self) -> ToolResult {
        let api = match crate::build_neboloop_api(&self.store) {
            Ok(a) => a,
            Err(e) => return ToolResult::error(format!("NeboLoop connection required: {}", e)),
        };

        let skills_resp = api.list_skills(None, None, Some(1), Some(100)).await;
        let skills = skills_resp.map(|r| r.skills).unwrap_or_default();

        if skills.is_empty() {
            return ToolResult::ok("No published artifacts on NeboLoop.");
        }

        let mut out = String::from("## Published Artifacts\n\n");
        for s in &skills {
            let vis = if s.is_installed { "installed" } else { &s.status };
            out.push_str(&format!("- **{}** v{} [{}] — {}\n  ID: `{}`\n",
                s.name, s.version, vis, s.description, s.id));
        }
        ToolResult::ok(out)
    }

    async fn handle_status(&self, input: &serde_json::Value) -> ToolResult {
        let id = input["id"].as_str().unwrap_or("");
        if id.is_empty() {
            return ToolResult::error("'id' is required (artifact ID from NeboLoop)");
        }

        let api = match crate::build_neboloop_api(&self.store) {
            Ok(a) => a,
            Err(e) => return ToolResult::error(format!("NeboLoop connection required: {}", e)),
        };

        match api.get_skill(id).await {
            Ok(detail) => {
                ToolResult::ok(format!(
                    "**{}** v{}\nStatus: {}\nType: {}\nCode: {}",
                    detail.item.name,
                    detail.item.version,
                    detail.item.status,
                    detail.artifact_type.as_deref().unwrap_or("unknown"),
                    detail.code.as_deref().unwrap_or("none"),
                ))
            }
            Err(e) => ToolResult::error(format!("Failed to fetch artifact: {}", e)),
        }
    }
}

impl DynTool for PublisherTool {
    fn name(&self) -> &str {
        "publisher"
    }

    fn description(&self) -> String {
        "Publish skills and agents to NeboLoop marketplace.\n\n\
         Actions:\n\
         - publish: publish a local skill or agent to NeboLoop\n\
         - list: list your published artifacts on NeboLoop\n\
         - status: check review/publication status of an artifact\n\n\
         EXAMPLES:\n  \
         publisher(action: \"publish\", type: \"agent\", name: \"marketing-manager\", version: \"1.0.0\", visibility: \"private\")\n  \
         publisher(action: \"publish\", type: \"skill\", name: \"seo-audit\", version: \"1.0.0\", visibility: \"public\")\n  \
         publisher(action: \"list\")\n  \
         publisher(action: \"status\", id: \"artifact-uuid\")\n\n\
         Visibility: \"private\" (only you), \"loop\" (shared with loops), \"public\" (marketplace, auto-submits for review)"
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "Action to perform",
                    "enum": ["publish", "list", "status"]
                },
                "type": {
                    "type": "string",
                    "description": "Artifact type (for publish)",
                    "enum": ["agent", "skill"]
                },
                "name": {
                    "type": "string",
                    "description": "Local agent or skill name (for publish)"
                },
                "version": {
                    "type": "string",
                    "description": "Version string (for publish, default: 1.0.0)"
                },
                "visibility": {
                    "type": "string",
                    "description": "Visibility: private (default), loop, or public (for publish)",
                    "enum": ["private", "loop", "public"]
                },
                "id": {
                    "type": "string",
                    "description": "NeboLoop artifact ID (for status)"
                }
            },
            "required": ["action"]
        })
    }

    fn requires_approval(&self) -> bool {
        true // Publishing should require user approval
    }

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let action = input["action"].as_str().unwrap_or("");
            match action {
                "publish" => self.handle_publish(&input).await,
                "list" => self.handle_list().await,
                "status" => self.handle_status(&input).await,
                _ => ToolResult::error(format!(
                    "Unknown action '{}'. Available: publish, list, status", action
                )),
            }
        })
    }
}
