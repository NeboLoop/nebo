use std::sync::Arc;

use crate::domain::DomainInput;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

use crate::skills::{Loader, SkillSource};

/// SkillTool manages skills — SKILL.md-defined agent capabilities.
/// Delegates to the agent::skills::Loader for directory-based loading and hot-reload.
pub struct SkillTool {
    loader: Arc<Loader>,
    store: Option<Arc<db::Store>>,
}

impl SkillTool {
    pub fn new(loader: Arc<Loader>) -> Self {
        Self { loader, store: None }
    }

    pub fn with_store(mut self, store: Arc<db::Store>) -> Self {
        self.store = Some(store);
        self
    }

    fn user_skills_dir() -> Result<std::path::PathBuf, String> {
        config::user_dir()
            .map(|d| d.join("skills"))
            .map_err(|e| format!("data dir error: {}", e))
    }
}

impl DynTool for SkillTool {
    fn name(&self) -> &str {
        "skill"
    }

    fn description(&self) -> String {
        "Capabilities & knowledge — skill catalog, loading, and execution.\n\
         USE THIS when: user asks for something unfamiliar, or you're unsure if a specialized skill exists for the task.\n\n\
         Before replying to any request, scan your available skills:\n\
         1. If a skill clearly applies → load it with skill(name: \"...\") to get detailed instructions, then follow them\n\
         2. If multiple skills could apply → choose the most specific one\n\
         3. If no skill applies → proceed with your built-in tools\n\n\
         - skill(action: \"catalog\") — Browse all available skills and apps\n\
         - skill(action: \"help\", name: \"calendar\") — Show full content of a skill\n\
         - skill(name: \"calendar\", resource: \"events\", action: \"list\") — Execute a skill action directly\n\
         - skill(action: \"browse\", name: \"xlsx-processor\") — List resource files in a skill's directory\n\
         - skill(action: \"read_resource\", name: \"xlsx-processor\", path: \"scripts/recalc.py\") — Read a resource file\n\
         - skill(action: \"load\", name: \"coding-assistant\") — Activate for current session\n\
         - skill(action: \"install\", code: \"SKIL-XXXX-XXXX\") — Install from marketplace\n\
         - skill(action: \"configure\", name: \"brave-search\", key: \"BRAVE_API_KEY\", value: \"...\") — Set a secret\n\
         - skill(action: \"discover\", query: \"email management\") — Search for skills matching a description\n\
         - skill(action: \"featured\") / skill(action: \"popular\") / skill(action: \"reviews\", name: \"...\")\n\n\
         If you're about to do something and aren't sure if a skill exists for it, call skill(action: \"discover\", query: \"what you're trying to do\") to check.\n\
         If a skill returns an auth error, guide the user to Settings → Apps to reconnect.\n\n\
         GUARDRAILS: Only invoke skills that appear in the catalog or discover results. Do not guess skill names."
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "Action to perform",
                    "enum": ["catalog", "discover", "help", "browse", "read_resource", "load", "unload", "create", "update", "delete", "install", "configure", "secrets", "featured", "popular", "reviews"]
                },
                "name": {
                    "type": "string",
                    "description": "Skill name (slug)"
                },
                "content": {
                    "type": "string",
                    "description": "Skill YAML content (for create/update)"
                },
                "path": {
                    "type": "string",
                    "description": "Relative path for browse filter or resource read"
                },
                "code": {
                    "type": "string",
                    "description": "Marketplace code for install (e.g. SKIL-XXXX-XXXX)"
                },
                "key": {
                    "type": "string",
                    "description": "Secret/API key name for configure action (e.g. BRAVE_API_KEY)"
                },
                "value": {
                    "type": "string",
                    "description": "Secret value for configure action"
                },
                "query": {
                    "type": "string",
                    "description": "Search query for discover action (describe what you're trying to do)"
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
                                let source_label = match s.source {
                                    SkillSource::Installed => "nebo",
                                    SkillSource::User => "user",
                                };
                                let triggers = if s.triggers.is_empty() {
                                    String::new()
                                } else {
                                    format!(" (triggers: {})", s.triggers.join(", "))
                                };
                                let caps = if s.capabilities.is_empty() {
                                    String::new()
                                } else {
                                    format!(" [caps: {}]", s.capabilities.join(", "))
                                };
                                let resource_count = s.list_resources().map(|r| r.len()).unwrap_or(0);
                                let resources = if resource_count > 0 {
                                    format!(" ({} resource files)", resource_count)
                                } else {
                                    String::new()
                                };
                                format!("- {} [{}|{}] — {}{}{}{}", s.name, status, source_label, s.description, caps, resources, triggers)
                            })
                            .collect();
                        ToolResult::ok(format!(
                            "{} skills:\n{}",
                            skills.len(),
                            lines.join("\n")
                        ))
                    }
                }
                "discover" => {
                    let query = input["query"].as_str().unwrap_or("");
                    if query.is_empty() {
                        return ToolResult::error("query is required — describe what you're trying to do");
                    }
                    let matches = self.loader.discover(query).await;
                    if matches.is_empty() {
                        ToolResult::ok(format!("No skills match \"{}\". Try a different query or check the catalog.", query))
                    } else {
                        let lines: Vec<String> = matches.iter().take(10).map(|s| {
                            format!("- **{}** — {}", s.name, s.description)
                        }).collect();
                        ToolResult::ok(format!(
                            "Skills matching \"{}\":\n{}\n\nTo use a skill, call: skill(action: \"help\", name: \"<name>\") for full instructions.",
                            query, lines.join("\n")
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
                            if !skill.capabilities.is_empty() {
                                output.push_str(&format!("**Capabilities:** {}\n", skill.capabilities.join(", ")));
                            }
                            if !skill.template.is_empty() {
                                output.push_str(&format!("\n---\n\n{}", skill.template));
                            }
                            // Append resource info
                            if let Ok(resources) = skill.list_resources() {
                                if !resources.is_empty() {
                                    output.push_str(&format!("\n\n---\n\n**Resources:** {} files\n", resources.len()));
                                    // Show available subdirectories
                                    let mut dirs: Vec<String> = resources
                                        .iter()
                                        .filter_map(|r| r.split('/').next().map(String::from))
                                        .collect::<std::collections::HashSet<_>>()
                                        .into_iter()
                                        .filter(|d| resources.iter().any(|r| r.starts_with(&format!("{}/", d))))
                                        .collect();
                                    dirs.sort();
                                    if !dirs.is_empty() {
                                        output.push_str(&format!("**Directories:** {}\n", dirs.join(", ")));
                                    }
                                    output.push_str("\nUse skill(action: \"browse\", name: \"");
                                    output.push_str(name);
                                    output.push_str("\") to explore resources.");
                                }
                            }
                            ToolResult::ok(output)
                        }
                        None => {
                            // Fall back to reading raw file from skills dir
                            let dir = match Self::user_skills_dir() {
                                Ok(d) => d,
                                Err(e) => return ToolResult::error(e),
                            };
                            let skill_md_path = dir.join(name).join("SKILL.md");
                            let skill_md_disabled = dir.join(name).join("SKILL.md.disabled");
                            let path = if skill_md_path.exists() {
                                skill_md_path
                            } else if skill_md_disabled.exists() {
                                skill_md_disabled
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
                "browse" => {
                    let name = input["name"].as_str().unwrap_or("");
                    if name.is_empty() {
                        return ToolResult::error("name is required");
                    }
                    let filter_path = input["path"].as_str().unwrap_or("");

                    match self.loader.get(name).await {
                        Some(skill) => match skill.list_resources() {
                            Ok(mut resources) => {
                                if !filter_path.is_empty() {
                                    let prefix = if filter_path.ends_with('/') {
                                        filter_path.to_string()
                                    } else {
                                        format!("{}/", filter_path)
                                    };
                                    resources.retain(|r| r.starts_with(&prefix));
                                }
                                if resources.is_empty() {
                                    if filter_path.is_empty() {
                                        ToolResult::ok(format!("Skill '{}' has no resource files.", name))
                                    } else {
                                        ToolResult::ok(format!("No resources found in '{}/{}'.", name, filter_path))
                                    }
                                } else {
                                    resources.sort();
                                    let listing: Vec<String> = resources.iter().map(|r| {
                                        let size = if let Some(ref base) = skill.base_dir {
                                            std::fs::metadata(base.join(r))
                                                .map(|m| format!(" ({} bytes)", m.len()))
                                                .unwrap_or_default()
                                        } else {
                                            String::new()
                                        };
                                        format!("  {}{}", r, size)
                                    }).collect();
                                    ToolResult::ok(format!(
                                        "Resources in '{}':\n{}",
                                        name,
                                        listing.join("\n")
                                    ))
                                }
                            }
                            Err(e) => ToolResult::error(format!("Failed to list resources: {}", e)),
                        },
                        None => ToolResult::error(format!("Skill '{}' not found", name)),
                    }
                }
                "read_resource" => {
                    let name = input["name"].as_str().unwrap_or("");
                    let path = input["path"].as_str().unwrap_or("");
                    if name.is_empty() || path.is_empty() {
                        return ToolResult::error("name and path are required");
                    }

                    match self.loader.get(name).await {
                        Some(skill) => match skill.read_resource(path) {
                            Ok(data) => {
                                match String::from_utf8(data.clone()) {
                                    Ok(text) => ToolResult::ok(text),
                                    Err(_) => ToolResult::ok(format!("binary file, {} bytes", data.len())),
                                }
                            }
                            Err(e) => ToolResult::error(e),
                        },
                        None => ToolResult::error(format!("Skill '{}' not found", name)),
                    }
                }
                "load" => {
                    let name = input["name"].as_str().unwrap_or("");
                    if name.is_empty() {
                        return ToolResult::error("name is required");
                    }
                    let dir = match Self::user_skills_dir() {
                        Ok(d) => d,
                        Err(e) => return ToolResult::error(e),
                    };
                    let skill_dir = dir.join(name);

                    if self.loader.get(name).await.is_some_and(|s| s.enabled) {
                        ToolResult::ok(format!("Skill '{}' is already enabled.", name))
                    } else if skill_dir.join("SKILL.md.disabled").exists() {
                        match std::fs::rename(
                            skill_dir.join("SKILL.md.disabled"),
                            skill_dir.join("SKILL.md"),
                        ) {
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
                    let dir = match Self::user_skills_dir() {
                        Ok(d) => d,
                        Err(e) => return ToolResult::error(e),
                    };
                    let skill_dir = dir.join(name);

                    if skill_dir.join("SKILL.md.disabled").exists() {
                        ToolResult::ok(format!("Skill '{}' is already disabled.", name))
                    } else if skill_dir.join("SKILL.md").exists() {
                        match std::fs::rename(
                            skill_dir.join("SKILL.md"),
                            skill_dir.join("SKILL.md.disabled"),
                        ) {
                            Ok(_) => ToolResult::ok(format!("Skill '{}' disabled.", name)),
                            Err(e) => ToolResult::error(format!("Failed to disable skill: {}", e)),
                        }
                    } else {
                        ToolResult::error(format!("Skill '{}' not found.", name))
                    }
                }
                "create" => {
                    let name = input["name"].as_str().unwrap_or("");
                    let content_raw = input["content"].as_str().unwrap_or("");

                    if name.is_empty() || content_raw.is_empty() {
                        return ToolResult::error("name and content are required");
                    }

                    // LLMs often send literal \n instead of real newlines in tool call strings.
                    let content = content_raw.replace("\\n", "\n");

                    let dir = match Self::user_skills_dir() {
                        Ok(d) => d,
                        Err(e) => return ToolResult::error(e),
                    };

                    // Always write as {name}/SKILL.md per Agent Skills spec
                    let final_content = if content.trim_start().starts_with("---") {
                        content.clone()
                    } else {
                        format!("---\nname: {}\ndescription: {}\n---\n{}", name, name, content)
                    };

                    let skill_dir = dir.join(name);
                    if let Err(e) = std::fs::create_dir_all(&skill_dir) {
                        return ToolResult::error(format!("Failed to create skill dir: {}", e));
                    }
                    let path = skill_dir.join("SKILL.md");
                    match std::fs::write(&path, final_content) {
                        Ok(_) => ToolResult::ok(format!("Created skill '{}' at {}", name, path.display())),
                        Err(e) => ToolResult::error(format!("Failed to write skill: {}", e)),
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
                        // Protect marketplace (installed) skills from modification
                        if matches!(skill.source, SkillSource::Installed) {
                            return ToolResult::error(format!(
                                "Cannot update marketplace skill '{}'. It was installed from NeboLoop and is read-only.",
                                name
                            ));
                        }
                        if let Some(ref path) = skill.source_path {
                            match std::fs::write(path, content) {
                                Ok(_) => return ToolResult::ok(format!("Updated skill '{}'", name)),
                                Err(e) => return ToolResult::error(format!("Failed to update: {}", e)),
                            }
                        }
                    }

                    let dir = match Self::user_skills_dir() {
                        Ok(d) => d,
                        Err(e) => return ToolResult::error(e),
                    };
                    let skill_md = dir.join(name).join("SKILL.md");
                    if !skill_md.exists() {
                        return ToolResult::error(format!("Skill '{}' not found", name));
                    }
                    match std::fs::write(&skill_md, content) {
                        Ok(_) => ToolResult::ok(format!("Updated skill '{}'", name)),
                        Err(e) => ToolResult::error(format!("Failed to update: {}", e)),
                    }
                }
                "delete" => {
                    let name = input["name"].as_str().unwrap_or("");
                    if name.is_empty() {
                        return ToolResult::error("name is required");
                    }

                    // Protect marketplace (installed) skills from deletion
                    if let Some(skill) = self.loader.get(name).await {
                        if matches!(skill.source, SkillSource::Installed) {
                            return ToolResult::error(format!(
                                "Cannot delete marketplace skill '{}'. It was installed from NeboLoop and is read-only.",
                                name
                            ));
                        }
                    }

                    let dir = match Self::user_skills_dir() {
                        Ok(d) => d,
                        Err(e) => return ToolResult::error(e),
                    };

                    let skill_dir = dir.join(name);
                    if skill_dir.is_dir() {
                        if let Err(e) = std::fs::remove_dir_all(&skill_dir) {
                            tracing::warn!(skill = %name, error = %e, "failed to remove skill directory");
                        }
                    }

                    ToolResult::ok(format!("Deleted skill '{}'", name))
                }
                "featured" => {
                    // Return featured skills from the marketplace
                    // For now, filter installed skills that are enabled and have high capability counts
                    let skills = self.loader.list().await;
                    let featured: Vec<_> = skills.iter()
                        .filter(|s| s.enabled && !s.capabilities.is_empty())
                        .take(10)
                        .collect();
                    if featured.is_empty() {
                        ToolResult::ok("No featured skills available.")
                    } else {
                        let lines: Vec<String> = featured.iter()
                            .map(|s| format!("- {} — {} [caps: {}]", s.name, s.description, s.capabilities.join(", ")))
                            .collect();
                        ToolResult::ok(format!("Featured skills:\n{}", lines.join("\n")))
                    }
                }
                "popular" => {
                    // Return most-used skills (sorted by those that have triggers or capabilities)
                    let skills = self.loader.list().await;
                    let mut popular: Vec<_> = skills.iter()
                        .filter(|s| s.enabled)
                        .collect();
                    popular.sort_by(|a, b| b.capabilities.len().cmp(&a.capabilities.len()));
                    let top: Vec<_> = popular.into_iter().take(10).collect();
                    if top.is_empty() {
                        ToolResult::ok("No skills installed.")
                    } else {
                        let lines: Vec<String> = top.iter()
                            .map(|s| format!("- {} — {}", s.name, s.description))
                            .collect();
                        ToolResult::ok(format!("Popular skills:\n{}", lines.join("\n")))
                    }
                }
                "configure" => {
                    let name = input["name"].as_str().unwrap_or("");
                    let key = input["key"].as_str().unwrap_or("");
                    let value = input["value"].as_str().unwrap_or("");

                    if name.is_empty() || key.is_empty() || value.is_empty() {
                        return ToolResult::error("name, key, and value are required");
                    }

                    let store = match &self.store {
                        Some(s) => s,
                        None => return ToolResult::error("configure not available — store not configured"),
                    };

                    // Validate key name matches a declared secret in the skill
                    if let Some(skill) = self.loader.get(name).await {
                        let declarations = skill.secrets();
                        if !declarations.is_empty()
                            && !declarations.iter().any(|d| d.key == key)
                        {
                            let valid_keys: Vec<&str> =
                                declarations.iter().map(|d| d.key.as_str()).collect();
                            return ToolResult::error(format!(
                                "Unknown secret '{}' for skill '{}'. Declared secrets: {}",
                                key,
                                name,
                                valid_keys.join(", ")
                            ));
                        }
                    }

                    // Encrypt and store
                    let encrypted = match auth::credential::encrypt(value) {
                        Ok(v) => v,
                        Err(e) => return ToolResult::error(format!("encryption failed: {}", e)),
                    };

                    match store.set_skill_secret(name, key, &encrypted) {
                        Ok(()) => ToolResult::ok(format!(
                            "Configured {} for skill '{}'. The value is stored encrypted.",
                            key, name
                        )),
                        Err(e) => ToolResult::error(format!("failed to save secret: {}", e)),
                    }
                }
                "secrets" => {
                    let name = input["name"].as_str().unwrap_or("");
                    if name.is_empty() {
                        return ToolResult::error("name is required");
                    }

                    // Show declared secrets and their configuration status
                    let skill = match self.loader.get(name).await {
                        Some(s) => s,
                        None => return ToolResult::error(format!("Skill '{}' not found", name)),
                    };

                    let declarations = skill.secrets();
                    if declarations.is_empty() {
                        return ToolResult::ok(format!(
                            "Skill '{}' does not declare any secrets.",
                            name
                        ));
                    }

                    let store = match &self.store {
                        Some(s) => s,
                        None => return ToolResult::error("secrets not available — store not configured"),
                    };

                    let stored = store
                        .list_skill_secrets(name)
                        .unwrap_or_default();
                    let stored_keys: std::collections::HashSet<&str> =
                        stored.iter().map(|(k, _)| k.as_str()).collect();

                    let lines: Vec<String> = declarations
                        .iter()
                        .map(|d| {
                            let status = if stored_keys.contains(d.key.as_str()) {
                                "configured"
                            } else if d.required {
                                "MISSING (required)"
                            } else {
                                "not set (optional)"
                            };
                            let label = if d.label.is_empty() {
                                d.key.clone()
                            } else {
                                format!("{} ({})", d.label, d.key)
                            };
                            let hint = if d.hint.is_empty() {
                                String::new()
                            } else {
                                format!("\n    {}", d.hint)
                            };
                            format!("- {} [{}]{}", label, status, hint)
                        })
                        .collect();

                    ToolResult::ok(format!(
                        "Secrets for skill '{}':\n{}",
                        name,
                        lines.join("\n")
                    ))
                }
                "install" => {
                    let code = input["code"].as_str().unwrap_or("");
                    if code.is_empty() || !code.starts_with("SKIL-") {
                        return ToolResult::error("'code' is required and must start with SKIL- (e.g. SKIL-XXXX-XXXX)");
                    }

                    let store = match &self.store {
                        Some(s) => s,
                        None => return ToolResult::error("install not available — store not configured"),
                    };

                    let api = match crate::build_neboloop_api(store) {
                        Ok(a) => a,
                        Err(e) => return ToolResult::error(format!("NeboLoop connection required: {}", e)),
                    };

                    match api.install_skill(code).await {
                        Ok(resp) => {
                            if resp.status == "payment_required" {
                                return ToolResult::ok(format!(
                                    "Skill requires payment. Checkout: {}",
                                    resp.checkout_url.unwrap_or_default()
                                ));
                            }

                            let name = resp.artifact.name.clone();
                            let artifact_id = resp.artifact.id.clone();

                            // Fetch and persist artifact content
                            if let Err(e) = crate::persist_skill_from_api(&api, &artifact_id, &name, code).await {
                                tracing::warn!(code, error = %e, "failed to persist skill after install");
                            }

                            // Force reload so skill appears in catalog immediately
                            self.loader.load_all().await;

                            ToolResult::ok(format!("Installed skill: {}", name))
                        }
                        Err(e) => ToolResult::error(format!("install failed: {}", e)),
                    }
                }
                "reviews" => {
                    let name = input["name"].as_str().unwrap_or("");
                    if name.is_empty() {
                        return ToolResult::error("name is required for reviews");
                    }
                    // Reviews would come from the marketplace API; for now return placeholder
                    ToolResult::ok(format!("No reviews available for skill '{}'. Reviews are synced from the NeboLoop marketplace.", name))
                }
                other => ToolResult::error(format!(
                    "Unknown action: {}. Available: catalog, help, browse, read_resource, load, unload, create, update, delete, install, featured, popular, reviews",
                    other
                )),
            }
        })
    }
}
