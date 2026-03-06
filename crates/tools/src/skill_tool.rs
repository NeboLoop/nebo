use std::sync::Arc;

use crate::domain::DomainInput;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

use crate::skills::Loader;

/// SkillTool manages skills — SKILL.md-defined agent capabilities.
/// Delegates to the agent::skills::Loader for directory-based loading and hot-reload.
pub struct SkillTool {
    loader: Arc<Loader>,
}

impl SkillTool {
    pub fn new(loader: Arc<Loader>) -> Self {
        Self { loader }
    }

    fn skills_dir() -> Result<std::path::PathBuf, String> {
        config::data_dir()
            .map(|d| d.join("skills"))
            .map_err(|e| format!("data dir error: {}", e))
    }
}

impl DynTool for SkillTool {
    fn name(&self) -> &str {
        "skill"
    }

    fn description(&self) -> String {
        "Manage skills — browsable catalog of agent capabilities.\n\n\
         Actions:\n\
         - catalog: List all available skills\n\
         - help: Show full content of a skill by name\n\
         - load: Activate a skill for the current session\n\
         - unload: Deactivate a skill\n\
         - create: Create a new skill from YAML content\n\
         - update: Update an existing skill\n\
         - delete: Delete a user-created skill\n\n\
         Examples:\n  \
         skill(action: \"catalog\")\n  \
         skill(action: \"help\", name: \"coding-assistant\")\n  \
         skill(action: \"load\", name: \"coding-assistant\")"
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "Action to perform",
                    "enum": ["catalog", "help", "load", "unload", "create", "update", "delete"]
                },
                "name": {
                    "type": "string",
                    "description": "Skill name (slug)"
                },
                "content": {
                    "type": "string",
                    "description": "Skill YAML content (for create/update)"
                }
            },
            "required": ["action"]
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
            let domain_input: DomainInput = match serde_json::from_value(input.clone()) {
                Ok(v) => v,
                Err(e) => return ToolResult::error(format!("Failed to parse input: {}", e)),
            };

            match domain_input.action.as_str() {
                "catalog" | "list" => {
                    let skills = self.loader.list().await;
                    if skills.is_empty() {
                        ToolResult::ok("No skills installed. Create one with skill(action: \"create\", name: \"my-skill\", content: \"...\")")
                    } else {
                        let lines: Vec<String> = skills
                            .iter()
                            .map(|s| {
                                let status = if s.enabled { "enabled" } else { "disabled" };
                                let triggers = if s.triggers.is_empty() {
                                    String::new()
                                } else {
                                    format!(" (triggers: {})", s.triggers.join(", "))
                                };
                                format!("- {} [{}] — {}{}", s.name, status, s.description, triggers)
                            })
                            .collect();
                        ToolResult::ok(format!(
                            "{} skills:\n{}",
                            skills.len(),
                            lines.join("\n")
                        ))
                    }
                }
                "help" => {
                    let name = input["name"].as_str().unwrap_or("");
                    if name.is_empty() {
                        return ToolResult::error("name is required");
                    }

                    match self.loader.get(name).await {
                        Some(skill) => {
                            let mut output = format!("# Skill: {}\n\n", skill.name);
                            output.push_str(&format!("**Description:** {}\n", skill.description));
                            output.push_str(&format!("**Version:** {}\n", skill.version));
                            if !skill.triggers.is_empty() {
                                output.push_str(&format!("**Triggers:** {}\n", skill.triggers.join(", ")));
                            }
                            if !skill.tools.is_empty() {
                                output.push_str(&format!("**Tools:** {}\n", skill.tools.join(", ")));
                            }
                            if !skill.template.is_empty() {
                                output.push_str(&format!("\n---\n\n{}", skill.template));
                            }
                            ToolResult::ok(output)
                        }
                        None => {
                            // Fall back to reading raw file from skills dir
                            let dir = match Self::skills_dir() {
                                Ok(d) => d,
                                Err(e) => return ToolResult::error(e),
                            };
                            let yaml_path = dir.join(format!("{}.yaml", name));
                            let disabled_path = dir.join(format!("{}.yaml.disabled", name));
                            let path = if yaml_path.exists() {
                                yaml_path
                            } else if disabled_path.exists() {
                                disabled_path
                            } else {
                                return ToolResult::error(format!("Skill '{}' not found", name));
                            };
                            match std::fs::read_to_string(&path) {
                                Ok(content) => ToolResult::ok(format!("# Skill: {}\n\n{}", name, content)),
                                Err(e) => ToolResult::error(format!("Failed to read skill: {}", e)),
                            }
                        }
                    }
                }
                "load" => {
                    let name = input["name"].as_str().unwrap_or("");
                    if name.is_empty() {
                        return ToolResult::error("name is required");
                    }
                    let dir = match Self::skills_dir() {
                        Ok(d) => d,
                        Err(e) => return ToolResult::error(e),
                    };
                    let disabled_path = dir.join(format!("{}.yaml.disabled", name));
                    let enabled_path = dir.join(format!("{}.yaml", name));

                    if self.loader.get(name).await.is_some_and(|s| s.enabled) {
                        ToolResult::ok(format!("Skill '{}' is already enabled.", name))
                    } else if disabled_path.exists() {
                        match std::fs::rename(&disabled_path, &enabled_path) {
                            Ok(_) => ToolResult::ok(format!("Skill '{}' enabled.", name)),
                            Err(e) => ToolResult::error(format!("Failed to enable skill: {}", e)),
                        }
                    } else {
                        ToolResult::error(format!("Skill '{}' not found. Use skill(action: \"catalog\") to list available skills.", name))
                    }
                }
                "unload" => {
                    let name = input["name"].as_str().unwrap_or("");
                    if name.is_empty() {
                        return ToolResult::error("name is required");
                    }
                    let dir = match Self::skills_dir() {
                        Ok(d) => d,
                        Err(e) => return ToolResult::error(e),
                    };
                    let enabled_path = dir.join(format!("{}.yaml", name));
                    let disabled_path = dir.join(format!("{}.yaml.disabled", name));

                    if disabled_path.exists() {
                        ToolResult::ok(format!("Skill '{}' is already disabled.", name))
                    } else if enabled_path.exists() {
                        match std::fs::rename(&enabled_path, &disabled_path) {
                            Ok(_) => ToolResult::ok(format!("Skill '{}' disabled.", name)),
                            Err(e) => ToolResult::error(format!("Failed to disable skill: {}", e)),
                        }
                    } else {
                        ToolResult::error(format!("Skill '{}' not found.", name))
                    }
                }
                "create" => {
                    let name = input["name"].as_str().unwrap_or("");
                    let content = input["content"].as_str().unwrap_or("");

                    if name.is_empty() || content.is_empty() {
                        return ToolResult::error("name and content are required");
                    }

                    let dir = match Self::skills_dir() {
                        Ok(d) => d,
                        Err(e) => return ToolResult::error(e),
                    };

                    // Create as a SKILL.md directory if content has frontmatter, else .yaml
                    if content.trim_start().starts_with("---") {
                        let skill_dir = dir.join(name);
                        if let Err(e) = std::fs::create_dir_all(&skill_dir) {
                            return ToolResult::error(format!("Failed to create skill dir: {}", e));
                        }
                        let path = skill_dir.join("SKILL.md");
                        match std::fs::write(&path, content) {
                            Ok(_) => ToolResult::ok(format!("Created skill '{}' at {}", name, path.display())),
                            Err(e) => ToolResult::error(format!("Failed to write skill: {}", e)),
                        }
                    } else {
                        if let Err(e) = std::fs::create_dir_all(&dir) {
                            return ToolResult::error(format!("Failed to create skills dir: {}", e));
                        }
                        let path = dir.join(format!("{}.yaml", name));
                        match std::fs::write(&path, content) {
                            Ok(_) => ToolResult::ok(format!("Created skill '{}' at {}", name, path.display())),
                            Err(e) => ToolResult::error(format!("Failed to write skill: {}", e)),
                        }
                    }
                }
                "update" => {
                    let name = input["name"].as_str().unwrap_or("");
                    let content = input["content"].as_str().unwrap_or("");

                    if name.is_empty() || content.is_empty() {
                        return ToolResult::error("name and content are required");
                    }

                    // Check if skill exists in loader or as file
                    if let Some(skill) = self.loader.get(name).await {
                        if let Some(ref path) = skill.source_path {
                            match std::fs::write(path, content) {
                                Ok(_) => return ToolResult::ok(format!("Updated skill '{}'", name)),
                                Err(e) => return ToolResult::error(format!("Failed to update: {}", e)),
                            }
                        }
                    }

                    let dir = match Self::skills_dir() {
                        Ok(d) => d,
                        Err(e) => return ToolResult::error(e),
                    };
                    let path = dir.join(format!("{}.yaml", name));
                    if !path.exists() {
                        return ToolResult::error(format!("Skill '{}' not found", name));
                    }
                    match std::fs::write(&path, content) {
                        Ok(_) => ToolResult::ok(format!("Updated skill '{}'", name)),
                        Err(e) => ToolResult::error(format!("Failed to update: {}", e)),
                    }
                }
                "delete" => {
                    let name = input["name"].as_str().unwrap_or("");
                    if name.is_empty() {
                        return ToolResult::error("name is required");
                    }

                    let dir = match Self::skills_dir() {
                        Ok(d) => d,
                        Err(e) => return ToolResult::error(e),
                    };

                    // Delete SKILL.md directory
                    let skill_dir = dir.join(name);
                    if skill_dir.is_dir() {
                        if let Err(e) = std::fs::remove_dir_all(&skill_dir) {
                            tracing::warn!(skill = %name, error = %e, "failed to remove skill directory");
                        }
                    }

                    // Delete .yaml files
                    let yaml_path = dir.join(format!("{}.yaml", name));
                    let disabled_path = dir.join(format!("{}.yaml.disabled", name));
                    if yaml_path.exists() {
                        if let Err(e) = std::fs::remove_file(&yaml_path) {
                            tracing::warn!(path = %yaml_path.display(), error = %e, "failed to remove skill yaml");
                        }
                    }
                    if disabled_path.exists() {
                        if let Err(e) = std::fs::remove_file(&disabled_path) {
                            tracing::warn!(path = %disabled_path.display(), error = %e, "failed to remove disabled skill yaml");
                        }
                    }

                    ToolResult::ok(format!("Deleted skill '{}'", name))
                }
                other => ToolResult::error(format!(
                    "Unknown action: {}. Available: catalog, help, load, unload, create, update, delete",
                    other
                )),
            }
        })
    }
}
