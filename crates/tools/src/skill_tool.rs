use std::sync::Arc;

use crate::domain::DomainInput;
use crate::errors;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

use crate::skills::{Loader, SkillSource};

/// SkillTool manages skills — SKILL.md-defined agent capabilities.
/// Delegates to the agent::skills::Loader for directory-based loading and hot-reload.
pub struct SkillTool {
    loader: Arc<Loader>,
    store: Option<Arc<db::Store>>,
    /// Optional reference to the live plugin registry. When set, skill
    /// discover/help can detect when the LLM has confused a plugin slug
    /// for a skill name and redirect to the `plugin` tool instead of
    /// returning a dead "not found." Runtime-driven — no hardcoded slugs.
    plugin_store: Option<Arc<napp::plugin::PluginStore>>,
    /// Shared canonical-installer cell (server-injected). `install` delegates here so it
    /// goes through the ONE `codes::handle_code` pathway — never a direct API bypass.
    code_installer: Arc<std::sync::RwLock<Option<Arc<dyn crate::bot_tool::CodeInstaller>>>>,
}

impl SkillTool {
    pub fn new(loader: Arc<Loader>) -> Self {
        Self {
            loader,
            store: None,
            plugin_store: None,
            code_installer: Arc::new(std::sync::RwLock::new(None)),
        }
    }

    pub fn with_store(mut self, store: Arc<db::Store>) -> Self {
        self.store = Some(store);
        self
    }

    /// Inject the shared canonical-installer cell (from the `Registry`).
    pub fn with_code_installer(
        mut self,
        installer: Arc<std::sync::RwLock<Option<Arc<dyn crate::bot_tool::CodeInstaller>>>>,
    ) -> Self {
        self.code_installer = installer;
        self
    }

    pub fn with_plugin_store(mut self, plugin_store: Arc<napp::plugin::PluginStore>) -> Self {
        self.plugin_store = Some(plugin_store);
        self
    }

    /// Find an installed plugin whose slug matches `term` (case-insensitive
    /// exact or substring). Returns the canonical slug if matched.
    fn match_plugin_slug(&self, term: &str) -> Option<String> {
        let store = self.plugin_store.as_ref()?;
        let needle = term.trim().to_lowercase();
        if needle.is_empty() {
            return None;
        }
        let mut exact: Option<String> = None;
        let mut substring: Option<String> = None;
        for (slug, _ver, _path, _src) in store.list_installed() {
            let lower = slug.to_lowercase();
            if lower == needle {
                exact = Some(slug);
                break;
            }
            if substring.is_none() && (lower.contains(&needle) || needle.contains(&lower)) {
                substring = Some(slug);
            }
        }
        exact.or(substring)
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
         NEVER USE this tool for channel messaging (slack/discord/teams/etc.). \
         Channels are PLUGINS, not skills — route channel I/O through the `plugin` tool with the channel name as `resource`. \
         `skill discover` will not find any channel by name; `skill help` will not return a channel's commands.\n\n\
         Before replying to any request, scan your available skills:\n\
         1. If a skill clearly applies → load it with skill(name: \"...\") to get detailed instructions, then follow them\n\
         2. If multiple skills could apply → choose the most specific one\n\
         3. If no skill applies → proceed with your built-in tools\n\n\
         - skill(action: \"list\") — Browse all available skills and apps\n\
         - skill(action: \"help\", name: \"calendar\") — Show full content of a skill\n\
         - skill(name: \"calendar\", resource: \"events\", action: \"list\") — Execute a skill action directly\n\
         - skill(action: \"browse\", name: \"xlsx-processor\") — List resource files in a skill's directory\n\
         - skill(action: \"read_resource\", name: \"xlsx-processor\", path: \"scripts/recalc.py\") — Read a resource file\n\
         - skill(action: \"load\", name: \"coding-assistant\") — Activate for current session\n\
         - skill(action: \"install\", code: \"SKIL-XXXX-XXXX\") — Install from marketplace\n\
         - skill(action: \"configure\", name: \"brave-search\", key: \"BRAVE_API_KEY\", value: \"...\") — Set a secret\n\
         - skill(action: \"discover\", query: \"email management\") — Search for skills matching a description\n\
         - skill(action: \"reviews\", name: \"...\") — read reviews for a skill\n\
         - skill(action: \"rate\", name: \"...\", rating: 5, review: \"It saved me a ton of time\") — Leave a 1–5 ★ review on a marketplace skill\n\n\
         If you're about to do something and aren't sure if a skill exists for it, call skill(action: \"discover\", query: \"what you're trying to do\") to check.\n\
         If a skill returns an auth error, guide the user to Settings → Apps to reconnect.\n\n\
         GUARDRAILS: Only invoke skills that appear in the list or discover results. Do not guess skill names."
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "Action to perform",
                    "enum": ["list", "discover", "help", "browse", "read_resource", "load", "unload", "create", "update", "delete", "install", "configure", "secrets", "reviews", "rate"]
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
                },
                "rating": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 5,
                    "description": "Star rating 1–5 (for rate action)"
                },
                "review": {
                    "type": "string",
                    "description": "Free-text review body (for rate action). Keep it honest and useful — what worked, what didn't."
                }
            },
            "required": ["action"]
        })
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn is_concurrent_safe(&self, input: &serde_json::Value) -> bool {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
        matches!(action, "list" | "discover" | "help" | "browse" | "read_resource" | "reviews" | "secrets")
        // `rate` is intentionally excluded — it mutates marketplace state.
    }

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let domain_input: DomainInput = match serde_json::from_value(input.clone()) {
                Ok(v) => v,
                Err(e) => return ToolResult::error(format!("Failed to parse input: {}. Do not retry — this is a schema error.", e)),
            };

            match domain_input.action.as_str() {
                "list" => {
                    // Budget-constrained catalog: show count +
                    // capped entries with truncated descriptions. Never dump the full
                    // catalog — use discover(query) for targeted search.
                    const MAX_CATALOG_ENTRIES: usize = 30;
                    const MAX_DESC_CHARS: usize = 120;

                    let skills = self.loader.list_summaries().await;
                    if skills.is_empty() {
                        ToolResult::ok(
                            "No skills installed. Create one with skill(action: \"create\", name: \"my-skill\", content: \"...\")",
                        )
                    } else {
                        let total = skills.len();
                        let enabled = skills.iter().filter(|s| s.enabled).count();
                        let lines: Vec<String> = skills
                            .iter()
                            .filter(|s| s.enabled)
                            .take(MAX_CATALOG_ENTRIES)
                            .map(|s| {
                                let mut desc = s.description.clone();
                                if desc.len() > MAX_DESC_CHARS {
                                    desc.truncate(MAX_DESC_CHARS);
                                    desc.push_str("...");
                                }
                                format!("- **{}** — {}", s.name, desc)
                            })
                            .collect();
                        let shown = lines.len();
                        let mut result = format!("{} skills ({} enabled):\n{}", total, enabled, lines.join("\n"));
                        if enabled > shown {
                            result.push_str(&format!(
                                "\n\n... and {} more. Use skill(action: \"discover\", query: \"...\") to search.",
                                enabled - shown
                            ));
                        }
                        result.push_str("\n\nTo use a skill: skill(action: \"load\", name: \"<name>\") to load its full instructions, then follow them inline. (help = a short metadata preview; load = the actual instructions.)");
                        ToolResult::ok(result)
                    }
                }
                "discover" => {
                    let query = input["query"].as_str().unwrap_or("");
                    if query.is_empty() {
                        return ToolResult::error(errors::missing_param(
                            "discover",
                            "query",
                            "skill(action: \"discover\", query: \"email management\")",
                        ));
                    }
                    let matches = self.loader.discover_summaries(query).await;
                    if matches.is_empty() {
                        // If the query matches a registered plugin slug
                        // (channel plugins like slack/discord, or any
                        // other installed plugin), the LLM probably
                        // meant to call the plugin tool. Redirect rather
                        // than returning a dead "no skills match."
                        if let Some(slug) = self.match_plugin_slug(query) {
                            return ToolResult::ok(format!(
                                "`{}` is a plugin, not a skill. Skills are local capability bundles; plugins are managed binaries. \
                                 USE: plugin(resource: \"{}\", action: \"exec\", command: \"help\") to see its commands, \
                                 then call plugin(resource: \"{}\", command: \"<subcommand> ...\") to use it. \
                                 For channel messaging (upload/post/dm/reply), the bridge fills channel and thread from context — you only need the operation and its arguments.",
                                slug, slug, slug
                            ));
                        }
                        {
                        ToolResult::error(format!(
                            "No skills or plugins found for \"{}\". \
                             This capability is not available. \
                             Report this to the user and suggest they install a skill from the marketplace. \
                             Do NOT attempt to perform the task through the browser or shell — it will not work.",
                            query
                        ))
                    }
                    } else {
                        let lines: Vec<String> = matches
                            .iter()
                            .take(10)
                            .map(|s| format!("- **{}** — {}", s.name, s.description))
                            .collect();
                        ToolResult::ok(format!(
                            "Skills matching \"{}\":\n{}\n\nTo use a skill, call: skill(action: \"load\", name: \"<name>\") to load its full instructions, then follow them inline.",
                            query,
                            lines.join("\n")
                        ))
                    }
                }
                "help" => {
                    let name = input["name"].as_str().unwrap_or("");
                    if name.is_empty() {
                        return ToolResult::error(errors::missing_param(
                            "help",
                            "name",
                            "skill(action: \"help\", name: \"calendar\")",
                        ));
                    }

                    match self.loader.get(name).await {
                        Some(skill) => {
                            let mut output = format!("# Skill: {}\n\n", skill.name);
                            output.push_str(&format!("**Description:** {}\n", skill.description));
                            output.push_str(&format!("**Version:** {}\n", skill.version));
                            if !skill.triggers.is_empty() {
                                output.push_str(&format!(
                                    "**Triggers:** {}\n",
                                    skill.triggers.join(", ")
                                ));
                            }
                            if !skill.capabilities.is_empty() {
                                output.push_str(&format!(
                                    "**Capabilities:** {}\n",
                                    skill.capabilities.join(", ")
                                ));
                            }
                            if !skill.plugins.is_empty() {
                                let slug = &skill.plugins[0].name;
                                output.push_str(&format!(
                                    "\n**Execute via:** `plugin(resource: \"{}\", action: \"exec\", command: \"...\")`\n\
                                     Do NOT use os/shell. The plugin tool handles auth and binary resolution.\n",
                                    slug
                                ));
                            }
                            // `help` is a lightweight inspect — it does NOT dump the full
                            // skill body into the conversation. To actually load and follow
                            // the skill's instructions, use skill(action: "load").
                            if !skill.template.is_empty() {
                                output.push_str(
                                    "\nTo load and follow this skill's full instructions, use skill(action: \"load\", name: \"...\").\n",
                                );
                            }
                            // Append resource info
                            if let Ok(resources) = skill.list_resources() {
                                if !resources.is_empty() {
                                    output.push_str(&format!(
                                        "\n\n---\n\n**Resources:** {} files\n",
                                        resources.len()
                                    ));
                                    // Show available subdirectories
                                    let mut dirs: Vec<String> = resources
                                        .iter()
                                        .filter_map(|r| r.split('/').next().map(String::from))
                                        .collect::<std::collections::HashSet<_>>()
                                        .into_iter()
                                        .filter(|d| {
                                            resources
                                                .iter()
                                                .any(|r| r.starts_with(&format!("{}/", d)))
                                        })
                                        .collect();
                                    dirs.sort();
                                    if !dirs.is_empty() {
                                        output.push_str(&format!(
                                            "**Directories:** {}\n",
                                            dirs.join(", ")
                                        ));
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
                                // The LLM may have confused a plugin slug
                                // (slack, discord, gws, ...) for a skill
                                // name. Redirect to the plugin tool rather
                                // than returning a dead "not found."
                                if let Some(slug) = self.match_plugin_slug(name) {
                                    return ToolResult::ok(format!(
                                        "`{}` is a plugin, not a skill. \
                                         USE: plugin(resource: \"{}\", action: \"exec\", command: \"help\") to see its commands, \
                                         then plugin(resource: \"{}\", command: \"<subcommand> ...\") to invoke them. \
                                         For channel messaging (upload/post/dm/reply), the bridge fills channel and thread from context.",
                                        slug, slug, slug
                                    ));
                                }
                                return ToolResult::error(format!("Skill '{}' not found", name));
                            };
                            match std::fs::read_to_string(&path) {
                                Ok(content) => {
                                    ToolResult::ok(format!("# Skill: {}\n\n{}", name, content))
                                }
                                Err(e) => ToolResult::error(format!("Failed to read skill: {}. Do not retry — this is a filesystem error.", e)),
                            }
                        }
                    }
                }
                "browse" => {
                    let name = input["name"].as_str().unwrap_or("");
                    if name.is_empty() {
                        return ToolResult::error(errors::missing_param(
                            "browse",
                            "name",
                            "skill(action: \"browse\", name: \"xlsx-processor\")",
                        ));
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
                                        ToolResult::ok(format!(
                                            "Skill '{}' has no resource files.",
                                            name
                                        ))
                                    } else {
                                        ToolResult::ok(format!(
                                            "No resources found in '{}/{}'.",
                                            name, filter_path
                                        ))
                                    }
                                } else {
                                    resources.sort();
                                    let listing: Vec<String> = resources
                                        .iter()
                                        .map(|r| {
                                            let size = if let Some(ref base) = skill.base_dir {
                                                std::fs::metadata(base.join(r))
                                                    .map(|m| format!(" ({} bytes)", m.len()))
                                                    .unwrap_or_default()
                                            } else {
                                                String::new()
                                            };
                                            format!("  {}{}", r, size)
                                        })
                                        .collect();
                                    ToolResult::ok(format!(
                                        "Resources in '{}':\n{}",
                                        name,
                                        listing.join("\n")
                                    ))
                                }
                            }
                            Err(e) => ToolResult::error(format!("Failed to list resources: {}. Do not retry — this is a filesystem error.", e)),
                        },
                        None => ToolResult::error(format!("Skill '{}' not found", name)),
                    }
                }
                "read_resource" => {
                    let name = input["name"].as_str().unwrap_or("");
                    let path = input["path"].as_str().unwrap_or("");
                    if name.is_empty() || path.is_empty() {
                        return ToolResult::error(errors::missing_param(
                            "read_resource",
                            "name and path",
                            "skill(action: \"read_resource\", name: \"xlsx-processor\", path: \"scripts/recalc.py\")",
                        ));
                    }

                    match self.loader.get(name).await {
                        Some(skill) => match skill.read_resource(path) {
                            Ok(data) => match String::from_utf8(data.clone()) {
                                Ok(text) => ToolResult::ok(text),
                                Err(_) => {
                                    ToolResult::ok(format!("binary file, {} bytes", data.len()))
                                }
                            },
                            Err(e) => ToolResult::error(e),
                        },
                        None => ToolResult::error(format!("Skill '{}' not found", name)),
                    }
                }
                "load" => {
                    let name = input["name"].as_str().unwrap_or("");
                    if name.is_empty() {
                        return ToolResult::error(errors::missing_param(
                            "load",
                            "name",
                            "skill(action: \"load\", name: \"coding-assistant\")",
                        ));
                    }
                    // Canonical "give me this skill's instructions": if enabled, return
                    // its expanded body so you can follow it now (loaded inline, rides in
                    // message history, unloads via the sliding window).
                    if let Some(skill) = self.loader.get(name).await {
                        if skill.enabled {
                            let body = self.loader.expand_template(&skill, self.store.as_deref());
                            return ToolResult::ok(format!(
                                "Loaded skill '{}'. Follow these instructions:\n\n{}",
                                skill.name, body
                            ));
                        }
                    }
                    // Otherwise enable a disabled skill on disk (available next message).
                    let dir = match Self::user_skills_dir() {
                        Ok(d) => d,
                        Err(e) => return ToolResult::error(e),
                    };
                    let skill_dir = dir.join(name);
                    if skill_dir.join("SKILL.md.disabled").exists() {
                        match std::fs::rename(
                            skill_dir.join("SKILL.md.disabled"),
                            skill_dir.join("SKILL.md"),
                        ) {
                            Ok(_) => ToolResult::ok(format!(
                                "Enabled skill '{}'. It will be available on your next message.",
                                name
                            )),
                            Err(e) => ToolResult::error(format!("Failed to enable skill: {}. Do not retry — this is a filesystem error.", e)),
                        }
                    } else {
                        ToolResult::error(format!(
                            "Skill '{}' not found. Use skill(action: \"list\") to list available skills.",
                            name
                        ))
                    }
                }
                "unload" => {
                    let name = input["name"].as_str().unwrap_or("");
                    if name.is_empty() {
                        return ToolResult::error(errors::missing_param(
                            "unload",
                            "name",
                            "skill(action: \"unload\", name: \"coding-assistant\")",
                        ));
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
                            Err(e) => ToolResult::error(format!("Failed to disable skill: {}. Do not retry — this is a filesystem error.", e)),
                        }
                    } else {
                        ToolResult::error(format!("Skill '{}' not found.", name))
                    }
                }
                "create" => {
                    let name = input["name"].as_str().unwrap_or("");
                    let content_raw = input["content"].as_str().unwrap_or("");

                    if name.is_empty() || content_raw.is_empty() {
                        return ToolResult::error(errors::missing_param(
                            "create",
                            "name and content",
                            "skill(action: \"create\", name: \"my-skill\", content: \"---\\nname: my-skill\\n---\\nInstructions here\")",
                        ));
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
                        format!(
                            "---\nname: {}\ndescription: {}\n---\n{}",
                            name, name, content
                        )
                    };

                    let skill_dir = dir.join(name);
                    if let Err(e) = std::fs::create_dir_all(&skill_dir) {
                        return ToolResult::error(format!("Failed to create skill dir: {}. Do not retry — this is a filesystem error.", e));
                    }
                    let path = skill_dir.join("SKILL.md");
                    match std::fs::write(&path, final_content) {
                        Ok(_) => {
                            // Make the skill (and its triggers) live NOW — the fs
                            // watcher is not instant and first-call trigger tests
                            // race it. Same pattern as the install paths.
                            self.loader.reload_from_disk().await;
                            ToolResult::ok(format!(
                                "Created skill '{}' at {}",
                                name,
                                path.display()
                            ))
                        }
                        Err(e) => ToolResult::error(format!("Failed to write skill: {}. Do not retry — this is a filesystem error.", e)),
                    }
                }
                "update" => {
                    let name = input["name"].as_str().unwrap_or("");
                    let content = input["content"].as_str().unwrap_or("");

                    if name.is_empty() || content.is_empty() {
                        return ToolResult::error(errors::missing_param(
                            "update",
                            "name and content",
                            "skill(action: \"update\", name: \"my-skill\", content: \"---\\nname: my-skill\\n---\\nUpdated instructions\")",
                        ));
                    }

                    // Check if skill exists in loader or as file
                    if let Some(skill) = self.loader.get(name).await {
                        // Protect marketplace (installed) skills from modification
                        if matches!(skill.source, SkillSource::Installed) {
                            return ToolResult::error(format!(
                                "Cannot update marketplace skill '{}'. It was installed from NeboAI and is read-only.",
                                name
                            ));
                        }
                        if let Some(ref path) = skill.source_path {
                            match std::fs::write(path, content) {
                                Ok(_) => {
                                    return ToolResult::ok(format!("Updated skill '{}'", name));
                                }
                                Err(e) => {
                                    return ToolResult::error(format!("Failed to update: {}. Do not retry — this is a filesystem error.", e));
                                }
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
                        Err(e) => ToolResult::error(format!("Failed to update: {}. Do not retry — this is a filesystem error.", e)),
                    }
                }
                "delete" => {
                    let name = input["name"].as_str().unwrap_or("");
                    if name.is_empty() {
                        return ToolResult::error(errors::missing_param(
                            "delete",
                            "name",
                            "skill(action: \"delete\", name: \"my-skill\")",
                        ));
                    }

                    // Protect marketplace (installed) skills from deletion
                    if let Some(skill) = self.loader.get(name).await {
                        if matches!(skill.source, SkillSource::Installed) {
                            return ToolResult::error(format!(
                                "Cannot delete marketplace skill '{}'. It was installed from NeboAI and is read-only.",
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
                "configure" => {
                    let name = input["name"].as_str().unwrap_or("");
                    let key = input["key"].as_str().unwrap_or("");
                    let value = input["value"].as_str().unwrap_or("");

                    if name.is_empty() || key.is_empty() || value.is_empty() {
                        return ToolResult::error(errors::missing_param(
                            "configure",
                            "name, key, and value",
                            "skill(action: \"configure\", name: \"brave-search\", key: \"BRAVE_API_KEY\", value: \"...\")",
                        ));
                    }

                    let store = match &self.store {
                        Some(s) => s,
                        None => {
                            return ToolResult::error(
                                "configure not available — store not configured. The user needs to restart Nebo so the database initializes.",
                            );
                        }
                    };

                    // Validate key name matches a declared secret in the skill
                    if let Some(skill) = self.loader.get(name).await {
                        let declarations = skill.secrets();
                        if !declarations.is_empty() && !declarations.iter().any(|d| d.key == key) {
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
                        Err(e) => return ToolResult::error(format!("encryption failed: {}. Do not retry — this is a configuration error.", e)),
                    };

                    match store.set_skill_secret(name, key, &encrypted) {
                        Ok(()) => ToolResult::ok(format!(
                            "Configured {} for skill '{}'. The value is stored encrypted.",
                            key, name
                        )),
                        Err(e) => ToolResult::error(format!("failed to save secret: {}. Do not retry — this is a database error.", e)),
                    }
                }
                "secrets" => {
                    let name = input["name"].as_str().unwrap_or("");
                    if name.is_empty() {
                        return ToolResult::error(errors::missing_param(
                            "secrets",
                            "name",
                            "skill(action: \"secrets\", name: \"brave-search\")",
                        ));
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
                        None => {
                            return ToolResult::error(
                                "secrets not available — store not configured. The user needs to restart Nebo so the database initializes.",
                            );
                        }
                    };

                    let stored = store.list_skill_secrets(name).unwrap_or_default();
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
                        return ToolResult::error(
                            "'code' is required and must start with SKIL- (e.g. SKIL-XXXX-XXXX)",
                        );
                    }
                    // Delegate to the ONE canonical install pathway (`codes::handle_code`):
                    // redeem + persist + reload + cascade deps, identical to the WS code flow.
                    // No direct API bypass.
                    let installer = self.code_installer.read().unwrap().clone();
                    match installer {
                        Some(installer) => ToolResult::ok(installer.install(code).await),
                        None => ToolResult::error(
                            "install requires the running app (no installer configured).",
                        ),
                    }
                }
                "reviews" => {
                    let name = input["name"].as_str().unwrap_or("");
                    if name.is_empty() {
                        return ToolResult::error(errors::missing_param(
                            "reviews",
                            "name",
                            "skill(action: \"reviews\", name: \"calendar\")",
                        ));
                    }
                    let store = match &self.store {
                        Some(s) => s,
                        None => {
                            return ToolResult::error(
                                "reviews not available — store not configured. The user needs to restart Nebo so the database initializes.",
                            );
                        }
                    };
                    let api = match crate::build_neboai_api(store) {
                        Ok(a) => a,
                        Err(e) => {
                            return ToolResult::error(format!(
                                "NeboAI connection required: {}",
                                e
                            ));
                        }
                    };
                    match api.get_skill_reviews(name, None, None).await {
                        Ok(resp) => {
                            if resp.reviews.is_empty() {
                                return ToolResult::ok(format!(
                                    "No reviews yet for skill '{}'.",
                                    name
                                ));
                            }
                            let lines: Vec<String> = resp
                                .reviews
                                .iter()
                                .map(|r| {
                                    let who = if r.reviewer_type == "bot" {
                                        // Bot slugs are already stored prefixed with `@`
                                        // (e.g. `@bot_xyz`). `/` is reserved for slash
                                        // commands — never use it for identities.
                                        format!("🤖 {}", if r.reviewer_name.is_empty() { r.reviewer_slug.clone() } else { r.reviewer_name.clone() })
                                    } else if !r.reviewer_name.is_empty() {
                                        r.reviewer_name.clone()
                                    } else {
                                        "Anonymous".to_string()
                                    };
                                    let stars = "★".repeat(r.rating as usize);
                                    format!("- {} {} — {}", who, stars, r.body)
                                })
                                .collect();
                            ToolResult::ok(format!(
                                "Reviews for skill '{}':\n{}",
                                name,
                                lines.join("\n")
                            ))
                        }
                        Err(e) => ToolResult::error(format!("failed to fetch reviews: {}. Do not retry — this is an API error.", e)),
                    }
                }
                "rate" => {
                    let name = input["name"].as_str().unwrap_or("");
                    let rating = input["rating"].as_i64().unwrap_or(0);
                    let review_body =
                        input["review"].as_str().unwrap_or("");
                    if name.is_empty() {
                        return ToolResult::error(errors::missing_param(
                            "rate",
                            "name",
                            "skill(action: \"rate\", name: \"calendar\", rating: 5, review: \"Great skill\")",
                        ));
                    }
                    if !(1..=5).contains(&rating) {
                        return ToolResult::error(errors::missing_param(
                            "rate",
                            "rating",
                            "skill(action: \"rate\", name: \"calendar\", rating: 5, review: \"Great skill\") — rating must be 1-5",
                        ));
                    }
                    let store = match &self.store {
                        Some(s) => s,
                        None => {
                            return ToolResult::error("rate not available — store not configured. The user needs to restart Nebo so the database initializes.");
                        }
                    };
                    let api = match crate::build_neboai_api(store) {
                        Ok(a) => a,
                        Err(e) => {
                            return ToolResult::error(format!(
                                "NeboAI connection required: {}",
                                e
                            ));
                        }
                    };
                    let body = serde_json::json!({ "rating": rating, "review": review_body });
                    match api.submit_skill_review(name, &body).await {
                        Ok(_) => ToolResult::ok(format!(
                            "Posted {}★ review on skill '{}'.",
                            rating, name
                        )),
                        Err(e) => ToolResult::error(format!("failed to post review: {}. Do not retry — this is an API error.", e)),
                    }
                }
                other => ToolResult::error(format!(
                    "Unknown action: {}. Available: list, discover, help, browse, read_resource, load, unload, create, update, delete, install, configure, secrets, reviews, rate",
                    other
                )),
            }
        })
    }
}
