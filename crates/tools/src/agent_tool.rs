use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::warn;

use db::Store;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// A single active agent — its own bot with isolated persona and scoped capabilities.
#[derive(Debug, Clone)]
pub struct ActiveAgent {
    /// Unique agent identifier (DB id or filesystem name).
    pub agent_id: String,
    /// Human-readable display name.
    pub name: String,
    /// Full AGENT.md body — becomes the system prompt identity.
    pub agent_md: String,
    /// Parsed agent.json config (workflows, skills, triggers).
    pub config: Option<napp::agent::AgentConfig>,
    /// Optional bound NeboLoop channel.
    pub channel_id: Option<String>,
}

/// Registry of all currently active agents. Multiple agents run concurrently.
/// Key = agent_id (lowercase name or DB id).
pub type AgentRegistry = Arc<RwLock<HashMap<String, ActiveAgent>>>;

/// Legacy alias — callers that only need the old behavior can still compile.
pub type ActiveAgentState = AgentRegistry;

/// PersonaTool manages the agent's personas — the top of the hierarchy.
/// A persona defines who the agent is: persona, workflows, skills, triggers.
pub struct PersonaTool {
    store: Arc<Store>,
    agent_registry: AgentRegistry,
    installed_dir: PathBuf,
    user_dir: PathBuf,
}

impl PersonaTool {
    pub fn new(store: Arc<Store>, agent_registry: AgentRegistry) -> Self {
        let data = config::data_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            store,
            agent_registry,
            installed_dir: data.join("nebo").join("agents"),
            user_dir: data.join("user").join("agents"),
        }
    }

    async fn handle_list(&self) -> ToolResult {
        // Scan filesystem
        let installed = napp::agent_loader::scan_installed_agents(&self.installed_dir);
        let user = napp::agent_loader::scan_user_agents(&self.user_dir);

        // Also check DB for agents
        let db_agents = self.store.list_agents(100, 0).unwrap_or_default();

        if installed.is_empty() && user.is_empty() && db_agents.is_empty() {
            return ToolResult::ok("No agents available.");
        }

        let mut lines = Vec::new();

        for agent in &installed {
            lines.push(format!(
                "- [installed] {} — {}",
                agent.agent_def.name,
                if agent.agent_def.description.is_empty() { "-" } else { &agent.agent_def.description }
            ));
        }
        for agent in &user {
            lines.push(format!(
                "- [user] {} — {}",
                agent.agent_def.name,
                if agent.agent_def.description.is_empty() { "-" } else { &agent.agent_def.description }
            ));
        }
        // Add DB-only agents not already in filesystem list
        let fs_names: Vec<&str> = installed.iter().chain(user.iter())
            .map(|r| r.agent_def.name.as_str())
            .collect();
        for agent in &db_agents {
            if !fs_names.contains(&agent.name.as_str()) {
                let enabled = if agent.is_enabled != 0 { "enabled" } else { "disabled" };
                lines.push(format!(
                    "- [db/{}] {} — {}",
                    enabled,
                    agent.name,
                    if agent.description.is_empty() { "-" } else { &agent.description }
                ));
            }
        }

        let registry = self.agent_registry.read().await;
        let active_count = registry.len();
        let status = if active_count > 0 {
            let names: Vec<&str> = registry.values().map(|r| r.name.as_str()).collect();
            format!(" ({} active: {})", active_count, names.join(", "))
        } else {
            String::new()
        };

        ToolResult::ok(format!("{} agents available{}:\n{}", lines.len(), status, lines.join("\n")))
    }

    async fn handle_activate(&self, input: &serde_json::Value) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("");
        if name.is_empty() {
            return ToolResult::error("'name' is required to activate an agent");
        }

        // Try loading from filesystem first
        let agent = self.find_agent(name);

        match agent {
            Some(loaded) => {
                let body = loaded.agent_def.body.clone();
                let agent_name = loaded.agent_def.name.clone();

                // Use the DB ID — every agent should have a DB entry
                let agent_id = if let Ok(agents) = self.store.list_agents(100, 0) {
                    if let Some(db_agent) = agents.iter().find(|r| r.name == agent_name) {
                        if db_agent.is_enabled == 0 {
                            let _ = self.store.toggle_agent(&db_agent.id);
                        }
                        db_agent.id.clone()
                    } else {
                        // No DB entry yet — create one
                        let id = uuid::Uuid::new_v4().to_string();
                        let frontmatter = loaded.config.as_ref()
                            .and_then(|c| serde_json::to_string(c).ok())
                            .unwrap_or_else(|| "{}".to_string());
                        match self.store.create_agent(&id, None, &agent_name, &loaded.agent_def.description, &body, &frontmatter, None, None) {
                            Ok(_) => {
                                let agent_dir = self.user_dir.join(&agent_name);
                                if agent_dir.exists() {
                                    let _ = self.store.set_agent_napp_path(&id, &agent_dir.to_string_lossy());
                                }
                            }
                            Err(e) => warn!(name = %agent_name, error = %e, "failed to create DB entry for agent"),
                        }
                        id
                    }
                } else if !loaded.agent_def.id.is_empty() {
                    loaded.agent_def.id.clone()
                } else {
                    uuid::Uuid::new_v4().to_string()
                };

                // Insert into agent registry (multiple agents can be active)
                let active = ActiveAgent {
                    agent_id: agent_id.clone(),
                    name: agent_name.clone(),
                    agent_md: body,
                    config: loaded.config.clone(),
                    channel_id: None,
                };
                self.agent_registry.write().await.insert(agent_id.clone(), active);

                let mut result = format!("Activated agent: {} (id: {})", agent_name, agent_id);
                if let Some(ref config) = loaded.config {
                    let wf_count = config.workflows.len();
                    let skill_count = config.skills.len();
                    if wf_count > 0 || skill_count > 0 {
                        result.push_str(&format!(
                            "\nDependencies: {} workflows, {} skills",
                            wf_count, skill_count
                        ));
                    }

                    // Register triggers (cron jobs, agent_workflows DB records)
                    self.register_config_triggers(&agent_id, config);
                }

                ToolResult::ok(result)
            }
            None => ToolResult::error(format!(
                "Agent '{}' not found. Use persona(action: \"list\") to see available agents.",
                name
            )),
        }
    }

    async fn handle_deactivate(&self, input: &serde_json::Value) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("");

        let mut registry = self.agent_registry.write().await;

        if name.is_empty() {
            // Deactivate all agents
            if registry.is_empty() {
                return ToolResult::ok("No agents are active.");
            }
            let names: Vec<String> = registry.values().map(|r| r.name.clone()).collect();
            registry.clear();
            ToolResult::ok(format!("Deactivated all agents: {}", names.join(", ")))
        } else {
            // Deactivate a specific agent by name or id
            let lower = name.to_lowercase();
            let key = registry.iter()
                .find(|(k, v)| k.to_lowercase() == lower || v.name.to_lowercase() == lower)
                .map(|(k, _)| k.clone());
            match key {
                Some(k) => {
                    let agent = registry.remove(&k).unwrap();
                    ToolResult::ok(format!("Deactivated agent: {}", agent.name))
                }
                None => ToolResult::error(format!(
                    "Agent '{}' is not active. Active agents: {}",
                    name,
                    if registry.is_empty() {
                        "none".to_string()
                    } else {
                        registry.values().map(|r| r.name.as_str()).collect::<Vec<_>>().join(", ")
                    }
                )),
            }
        }
    }

    async fn handle_info(&self, input: &serde_json::Value) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("");
        if name.is_empty() {
            // Show all active agents
            let registry = self.agent_registry.read().await;
            if registry.is_empty() {
                return ToolResult::ok("No agents are currently active.");
            }
            let mut lines = Vec::new();
            for (id, agent) in registry.iter() {
                let preview = if agent.agent_md.len() > 200 {
                    format!("{}...", crate::truncate_str(&agent.agent_md, 200))
                } else {
                    agent.agent_md.clone()
                };
                lines.push(format!("**{}** (id: {})\n{}", agent.name, id, preview));
            }
            return ToolResult::ok(format!("Active agents ({}):\n\n{}", registry.len(), lines.join("\n\n---\n\n")));
        }

        match self.find_agent(name) {
            Some(loaded) => {
                let version_str = loaded.version.as_deref().unwrap_or("-");
                let mut info = format!(
                    "Name: {}\nVersion: {}\nDescription: {}\nSource: {}\n",
                    loaded.agent_def.name,
                    version_str,
                    if loaded.agent_def.description.is_empty() { "-" } else { &loaded.agent_def.description },
                    match loaded.source {
                        napp::agent_loader::AgentSource::Installed => "marketplace",
                        napp::agent_loader::AgentSource::User => "user-created",
                    },
                );

                if let Some(ref config) = loaded.config {
                    if !config.workflows.is_empty() {
                        info.push_str("\nWorkflows:\n");
                        for (binding, wf) in &config.workflows {
                            let trigger_desc = match &wf.trigger {
                                napp::agent::AgentTrigger::Schedule { cron } => format!("schedule({})", cron),
                                napp::agent::AgentTrigger::Heartbeat { interval, window } => {
                                    match window {
                                        Some(w) => format!("heartbeat({}, {})", interval, w),
                                        None => format!("heartbeat({})", interval),
                                    }
                                }
                                napp::agent::AgentTrigger::Event { sources } => format!("event({})", sources.join(", ")),
                                napp::agent::AgentTrigger::Watch { plugin, command, .. } => format!("watch({}, {})", plugin, command),
                                napp::agent::AgentTrigger::Manual => "manual".to_string(),
                            };
                            let desc = if wf.description.is_empty() { "" } else { &wf.description };
                            let activities_note = if wf.has_activities() {
                                format!(" ({} activities)", wf.activities.len())
                            } else {
                                String::new()
                            };
                            info.push_str(&format!("  - {} [{}]{} {}\n", binding, trigger_desc, activities_note, desc));
                        }
                    }
                    if !config.skills.is_empty() {
                        info.push_str(&format!("\nSkills: {}\n", config.skills.join(", ")));
                    }
                    if let Some(ref pricing) = config.pricing {
                        info.push_str(&format!("\nPricing: {} (${:.2})\n", pricing.model, pricing.cost));
                    }
                }

                // Show AGENT.md body preview
                let body = &loaded.agent_def.body;
                let preview = if body.len() > 500 {
                    format!("{}...", crate::truncate_str(body, 500))
                } else {
                    body.clone()
                };
                info.push_str(&format!("\nPersona:\n{}", preview));

                ToolResult::ok(info)
            }
            None => ToolResult::error(format!("Agent '{}' not found.", name)),
        }
    }

    async fn handle_create(&self, input: &serde_json::Value) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("");
        if name.is_empty() {
            return ToolResult::error("'name' is required to create an agent");
        }

        let description = input["description"].as_str().unwrap_or("");

        // Build agent_json from structured automations, or use raw agent_json
        let agent_json_str = if let Some(autos) = input["automations"].as_array() {
            if autos.is_empty() {
                None
            } else {
                Some(Self::build_agent_json_from_automations(autos).to_string())
            }
        } else {
            input["agent_json"].as_str().map(|s| s.to_string())
                .or_else(|| {
                    let v = &input["agent_json"];
                    if v.is_object() { Some(v.to_string()) } else { None }
                })
        };

        // Auto-generate AGENT.md if not provided but name/description exist
        let agent_md_raw = input["agent_md"].as_str().unwrap_or("");
        let agent_md = if agent_md_raw.is_empty() {
            if description.is_empty() {
                return ToolResult::error("either 'agent_md' or 'description' is required to create an agent");
            }
            format!("---\nname: {}\ndescription: {}\n---\nYou are {}. {}", name, description, name, description)
        } else {
            // LLMs often send literal \n instead of real newlines in tool call strings.
            // Unescape so AGENT.md frontmatter parses correctly.
            agent_md_raw.replace("\\n", "\n")
        };

        let agent_dir = self.user_dir.join(name);
        if agent_dir.exists() {
            return ToolResult::error(format!("Agent '{}' already exists at {}", name, agent_dir.display()));
        }

        if let Err(e) = std::fs::create_dir_all(&agent_dir) {
            return ToolResult::error(format!("Failed to create directory: {}", e));
        }

        let agent_path = agent_dir.join("AGENT.md");
        if let Err(e) = std::fs::write(&agent_path, &agent_md) {
            return ToolResult::error(format!("Failed to write AGENT.md: {}", e));
        }

        // Write agent.json if provided (contains workflow bindings, triggers, skills, pricing)
        if let Some(ref rj) = agent_json_str {
            let _ = std::fs::write(agent_dir.join("agent.json"), rj);
        }

        // Auto-generate manifest.json so version info is available
        let manifest_path = agent_dir.join("manifest.json");
        if !manifest_path.exists() {
            let manifest = serde_json::json!({
                "name": name,
                "version": "1.0.0",
                "type": "agent",
                "description": description,
            });
            let _ = std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest).unwrap_or_default());
        }

        // Create DB entry so the agent has a proper UUID
        let id = uuid::Uuid::new_v4().to_string();
        let frontmatter = agent_json_str.as_deref().unwrap_or("{}");
        match self.store.create_agent(&id, None, name, description, &agent_md, frontmatter, None, None) {
            Ok(_) => {
                let _ = self.store.set_agent_napp_path(&id, &agent_dir.to_string_lossy());
            }
            Err(e) => {
                warn!(name, error = %e, "failed to create DB entry for agent");
            }
        }

        let mut result = format!("Created agent '{}' (id: {})", name, id);
        let mut has_heartbeat_or_event = false;

        // Parse config and register triggers
        let parsed_config = if let Some(ref rj) = agent_json_str {
            match napp::agent::parse_agent_config(rj) {
                Ok(config) => {
                    self.register_config_triggers(&id, &config);

                    // Describe what was registered
                    let trigger_descs: Vec<String> = config.workflows.iter().map(|(name, wf)| {
                        let t = match &wf.trigger {
                            napp::agent::AgentTrigger::Schedule { cron } => format!("schedule({})", cron),
                            napp::agent::AgentTrigger::Heartbeat { interval, window } => {
                                has_heartbeat_or_event = true;
                                match window {
                                    Some(w) => format!("heartbeat({}, {})", interval, w),
                                    None => format!("heartbeat({})", interval),
                                }
                            }
                            napp::agent::AgentTrigger::Event { sources } => {
                                has_heartbeat_or_event = true;
                                format!("event({})", sources.join(", "))
                            }
                            napp::agent::AgentTrigger::Watch { plugin, .. } => {
                                has_heartbeat_or_event = true;
                                format!("watch({})", plugin)
                            }
                            napp::agent::AgentTrigger::Manual => "manual".to_string(),
                        };
                        format!("{} [{}]", name, t)
                    }).collect();
                    if !trigger_descs.is_empty() {
                        result.push_str(&format!("\nAutomations: {}", trigger_descs.join(", ")));
                    }

                    Some(config)
                }
                Err(e) => {
                    result.push_str(&format!("\nWarning: agent.json parse error: {}", e));
                    None
                }
            }
        } else {
            None
        };

        // Auto-activate: insert into agent registry so it appears in sidebar immediately
        let active = ActiveAgent {
            agent_id: id.clone(),
            name: name.to_string(),
            agent_md: agent_md.clone(),
            config: parsed_config,
            channel_id: None,
        };
        self.agent_registry.write().await.insert(id.clone(), active);
        result.push_str("\nAgent activated and visible in sidebar.");

        if has_heartbeat_or_event {
            result.push_str("\nNote: heartbeat/event background loops start on server restart or via REST activate.");
        }

        ToolResult::ok(result)
    }

    async fn handle_update(&self, input: &serde_json::Value) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("");
        if name.is_empty() {
            return ToolResult::error("'name' is required to identify the agent to update");
        }

        // Find the agent in DB
        let db_agent = match self.store.list_agents(500, 0) {
            Ok(agents) => {
                let lower = name.to_lowercase();
                agents.into_iter().find(|r| r.name.to_lowercase() == lower || r.id == name)
            }
            Err(e) => return ToolResult::error(format!("Failed to query agents: {}", e)),
        };
        let db_agent = match db_agent {
            Some(r) => r,
            None => return ToolResult::error(format!("Agent '{}' not found. Use persona(action: \"list\") to see available agents.", name)),
        };

        let agent_id = &db_agent.id;
        let mut current_name = db_agent.name.clone();
        let mut current_desc = db_agent.description.clone();
        let mut current_md = db_agent.agent_md.clone();
        let mut current_frontmatter = db_agent.frontmatter.clone();
        let mut changes = Vec::new();

        // Update name (rename)
        if let Some(new_name) = input["new_name"].as_str() {
            if !new_name.is_empty() && new_name != current_name {
                // Rename filesystem directory if it exists
                let old_dir = self.user_dir.join(&current_name);
                let new_dir = self.user_dir.join(new_name);
                if old_dir.exists() {
                    if new_dir.exists() {
                        return ToolResult::error(format!("Cannot rename: '{}' already exists", new_name));
                    }
                    if let Err(e) = std::fs::rename(&old_dir, &new_dir) {
                        return ToolResult::error(format!("Failed to rename directory: {}", e));
                    }
                    let _ = self.store.set_agent_napp_path(agent_id, &new_dir.to_string_lossy());
                }
                changes.push(format!("renamed to '{}'", new_name));
                current_name = new_name.to_string();
            }
        }

        // Update description
        if let Some(desc) = input["description"].as_str() {
            if !desc.is_empty() {
                current_desc = desc.to_string();
                changes.push("description updated".to_string());
            }
        }

        // Update agent_md (persona)
        if let Some(md) = input["agent_md"].as_str() {
            if !md.is_empty() {
                current_md = md.replace("\\n", "\n");
                // Write to filesystem
                let agent_dir = self.user_dir.join(&current_name);
                if agent_dir.exists() {
                    let _ = std::fs::write(agent_dir.join("AGENT.md"), &current_md);
                }
                changes.push("persona (AGENT.md) updated".to_string());
            }
        }

        // Update input_values (user-supplied configuration values)
        if let Some(vals) = input.get("input_values") {
            if vals.is_object() {
                let vals_str = vals.to_string();
                match self.store.update_agent_input_values(agent_id, &vals_str) {
                    Ok(_) => changes.push("input values updated".to_string()),
                    Err(e) => changes.push(format!("failed to update input values: {}", e)),
                }
            }
        }

        // Update input schema (field definitions in agent.json)
        if let Some(schema) = input.get("inputs") {
            if schema.is_array() {
                let mut fm: serde_json::Value = serde_json::from_str(&current_frontmatter).unwrap_or(serde_json::json!({}));
                fm["inputs"] = schema.clone();
                current_frontmatter = fm.to_string();
                let agent_dir = self.user_dir.join(&current_name);
                if agent_dir.exists() {
                    let _ = std::fs::write(agent_dir.join("agent.json"), &current_frontmatter);
                }
                changes.push("input field schema updated".to_string());
            }
        }

        // toggle_automation: toggle a single binding on/off
        if let Some(binding_name) = input["toggle_automation"].as_str() {
            match self.store.toggle_agent_workflow(agent_id, binding_name) {
                Ok(new_state) => {
                    let state_str = if new_state { "enabled" } else { "disabled" };
                    changes.push(format!("automation '{}' {}", binding_name, state_str));
                }
                Err(e) => changes.push(format!("failed to toggle '{}': {}", binding_name, e)),
            }
        }

        // update_automation: update a single binding by name (non-destructive)
        if let Some(update_obj) = input.get("update_automation") {
            if let Some(binding_name) = update_obj["name"].as_str() {
                let mut fm: serde_json::Value = serde_json::from_str(&current_frontmatter).unwrap_or(serde_json::json!({}));

                if let Some(existing_binding) = fm.get_mut("workflows").and_then(|w| w.get_mut(binding_name)) {
                    // Merge individual fields into the existing binding
                    if let Some(desc) = update_obj["description"].as_str() {
                        existing_binding["description"] = serde_json::Value::String(desc.to_string());
                    }
                    if let Some(emit) = update_obj.get("emit") {
                        existing_binding["emit"] = emit.clone();
                    }
                    if let Some(steps) = update_obj["steps"].as_array() {
                        let activities: Vec<serde_json::Value> = steps.iter().enumerate().map(|(i, step)| {
                            let intent = step.as_str().unwrap_or("Execute step");
                            serde_json::json!({ "id": format!("step-{}", i + 1), "intent": intent })
                        }).collect();
                        existing_binding["activities"] = serde_json::Value::Array(activities);
                    }

                    // Update trigger if any trigger field is provided
                    let has_trigger_change = update_obj["schedule"].is_string()
                        || update_obj["interval"].is_string()
                        || !update_obj["sources"].is_null()
                        || update_obj["trigger"].is_string();
                    if has_trigger_change {
                        let trigger_type = if update_obj["schedule"].is_string() {
                            "schedule"
                        } else if update_obj["interval"].is_string() {
                            "heartbeat"
                        } else if !update_obj["sources"].is_null() {
                            "event"
                        } else {
                            update_obj["trigger"].as_str().unwrap_or("manual")
                        };
                        let trigger = match trigger_type {
                            "schedule" => {
                                let raw = update_obj["schedule"].as_str().unwrap_or("0 9 * * *");
                                let cron = Self::normalize_cron(raw);
                                serde_json::json!({ "type": "schedule", "cron": cron })
                            }
                            "heartbeat" => {
                                let interval = update_obj["interval"].as_str().unwrap_or("30m");
                                let mut t = serde_json::json!({ "type": "heartbeat", "interval": interval });
                                if let Some(window) = update_obj["window"].as_str() {
                                    t["window"] = serde_json::Value::String(window.to_string());
                                }
                                t
                            }
                            "event" => {
                                let sources: Vec<serde_json::Value> = if let Some(arr) = update_obj["sources"].as_array() {
                                    arr.clone()
                                } else if let Some(s) = update_obj["sources"].as_str() {
                                    s.split(',').map(|s| serde_json::Value::String(s.trim().to_string())).collect()
                                } else {
                                    vec![]
                                };
                                serde_json::json!({ "type": "event", "sources": sources })
                            }
                            _ => serde_json::json!({ "type": "manual" }),
                        };
                        existing_binding["trigger"] = trigger;

                        // Re-register trigger for this binding
                        let cron_name = format!("agent-{}-{}", agent_id, binding_name);
                        let _ = self.store.delete_cron_job_by_name(&cron_name);
                    }

                    current_frontmatter = fm.to_string();
                    let agent_dir = self.user_dir.join(&current_name);
                    if agent_dir.exists() {
                        let _ = std::fs::write(agent_dir.join("agent.json"), &current_frontmatter);
                    }

                    // Re-register triggers for the updated config
                    if let Ok(config) = napp::agent::parse_agent_config(&current_frontmatter) {
                        self.register_config_triggers(agent_id, &config);
                    }

                    // Upsert the workflow binding row in DB
                    if let Ok(config) = napp::agent::parse_agent_config(&current_frontmatter) {
                        if let Some(binding) = config.workflows.get(binding_name) {
                            let (trigger_type, trigger_config) = Self::flatten_trigger(&binding.trigger);
                            let activities_json = serde_json::to_string(&binding.activities).ok();
                            let inputs_json = if binding.inputs.is_empty() { None } else {
                                serde_json::to_string(&binding.inputs).ok()
                            };
                            let _ = self.store.upsert_agent_workflow(
                                agent_id, binding_name, &trigger_type, &trigger_config,
                                Some(&binding.description), inputs_json.as_deref(),
                                binding.emit.as_deref(), activities_json.as_deref(),
                            );
                        }
                    }

                    changes.push(format!("updated automation '{}'", binding_name));
                } else {
                    changes.push(format!("automation '{}' not found — use add_automations to create it", binding_name));
                }
            }
        }

        // Handle automations changes
        let mut automations_changed = false;

        // remove_automations: remove specific automations by name
        if let Some(removals) = input["remove_automations"].as_array() {
            for removal in removals {
                if let Some(binding_name) = removal.as_str() {
                    match self.store.delete_single_agent_workflow(agent_id, binding_name) {
                        Ok(_) => {
                            // Also remove cron job if it was a schedule trigger
                            let cron_name = format!("agent-{}-{}", agent_id, binding_name);
                            let _ = self.store.delete_cron_job_by_name(&cron_name);
                            changes.push(format!("removed automation '{}'", binding_name));
                            automations_changed = true;
                        }
                        Err(e) => {
                            changes.push(format!("failed to remove '{}': {}", binding_name, e));
                        }
                    }
                }
            }
        }

        // automations: replace ALL automations
        if let Some(autos) = input["automations"].as_array() {
            // Clear existing workflows and cron jobs
            let _ = self.store.delete_agent_workflows(agent_id);
            let cron_prefix = format!("agent-{}-", agent_id);
            let _ = self.store.delete_cron_jobs_by_prefix(&cron_prefix);

            if !autos.is_empty() {
                let agent_json = Self::build_agent_json_from_automations(autos);
                current_frontmatter = agent_json.to_string();

                // Write to filesystem
                let agent_dir = self.user_dir.join(&current_name);
                if agent_dir.exists() {
                    let _ = std::fs::write(agent_dir.join("agent.json"), &current_frontmatter);
                }

                if let Ok(config) = napp::agent::parse_agent_config(&current_frontmatter) {
                    self.register_config_triggers(agent_id, &config);
                    changes.push(format!("replaced all automations ({} total)", config.workflows.len()));
                }
            } else {
                current_frontmatter = "{}".to_string();
                changes.push("removed all automations".to_string());
            }
            automations_changed = true;
        }

        // add_automations: add new automations without removing existing ones
        if let Some(additions) = input["add_automations"].as_array() {
            if !additions.is_empty() {
                let new_json = Self::build_agent_json_from_automations(additions);
                if let Ok(config) = napp::agent::parse_agent_config(&new_json.to_string()) {
                    self.register_config_triggers(agent_id, &config);
                    let names: Vec<&str> = config.workflows.keys().map(|s| s.as_str()).collect();
                    changes.push(format!("added automations: {}", names.join(", ")));
                    automations_changed = true;
                }

                // Merge into frontmatter for DB storage
                let mut existing: serde_json::Value = serde_json::from_str(&current_frontmatter).unwrap_or(serde_json::json!({}));
                if let Some(new_wfs) = new_json["workflows"].as_object() {
                    let existing_wfs = existing["workflows"].as_object().cloned().unwrap_or_default();
                    let mut merged = existing_wfs;
                    for (k, v) in new_wfs {
                        merged.insert(k.clone(), v.clone());
                    }
                    existing["workflows"] = serde_json::Value::Object(merged);
                }
                current_frontmatter = existing.to_string();

                // Write merged agent.json to filesystem
                let agent_dir = self.user_dir.join(&current_name);
                if agent_dir.exists() {
                    let _ = std::fs::write(agent_dir.join("agent.json"), &current_frontmatter);
                }
            }
        }

        // Persist DB update
        if let Err(e) = self.store.update_agent(
            agent_id,
            &current_name,
            &current_desc,
            &current_md,
            &current_frontmatter,
            None,
            None,
        ) {
            return ToolResult::error(format!("Failed to update agent in DB: {}", e));
        }

        // Update live registry if agent is active
        let mut registry = self.agent_registry.write().await;
        if let Some(active) = registry.get_mut(agent_id) {
            active.name = current_name.clone();
            active.agent_md = current_md.clone();
            if automations_changed {
                active.config = napp::agent::parse_agent_config(&current_frontmatter).ok();
            }
            changes.push("live agent updated".to_string());
        }

        if changes.is_empty() {
            return ToolResult::ok(format!("No changes made to agent '{}'.", current_name));
        }

        ToolResult::ok(format!("Updated agent '{}' (id: {}):\n- {}", current_name, agent_id, changes.join("\n- ")))
    }

    async fn handle_delete(&self, input: &serde_json::Value) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("");
        if name.is_empty() {
            return ToolResult::error("'name' is required to delete an agent");
        }

        // Find in DB
        let db_agent = match self.store.list_agents(500, 0) {
            Ok(agents) => {
                let lower = name.to_lowercase();
                agents.into_iter().find(|r| r.name.to_lowercase() == lower || r.id == name)
            }
            Err(e) => return ToolResult::error(format!("Failed to query agents: {}", e)),
        };
        let db_agent = match db_agent {
            Some(r) => r,
            None => return ToolResult::error(format!("Agent '{}' not found.", name)),
        };

        let agent_id = &db_agent.id;
        let agent_name = &db_agent.name;

        // Remove from live registry
        self.agent_registry.write().await.remove(agent_id);

        // Delete cron jobs for this agent
        let cron_prefix = format!("agent-{}-", agent_id);
        let _ = self.store.delete_cron_jobs_by_prefix(&cron_prefix);

        // Delete agent workflows from DB
        let _ = self.store.delete_agent_workflows(agent_id);

        // Delete agent from DB
        if let Err(e) = self.store.delete_agent(agent_id) {
            return ToolResult::error(format!("Failed to delete agent from DB: {}", e));
        }

        // Remove filesystem directory (user-created only)
        let user_dir = self.user_dir.join(agent_name);
        if user_dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&user_dir) {
                return ToolResult::ok(format!(
                    "Deleted agent '{}' from DB and registry, but failed to remove directory {}: {}",
                    agent_name, user_dir.display(), e
                ));
            }
        }

        ToolResult::ok(format!("Deleted agent '{}' (id: {}). Removed from DB, registry, and filesystem.", agent_name, agent_id))
    }

    async fn handle_install(&self, input: &serde_json::Value) -> ToolResult {
        let code = input["code"].as_str().unwrap_or("");
        if code.is_empty() || !code.starts_with("AGNT-") {
            return ToolResult::error("'code' is required and must start with AGNT- (e.g. AGNT-ABCD-1234)");
        }

        // Check if already installed
        if let Ok(agents) = self.store.list_agents(100, 0) {
            if agents.iter().any(|r| r.kind.as_deref() == Some(code)) {
                return ToolResult::ok(format!("Agent {} is already installed.", code));
            }
        }

        let api = match crate::build_neboloop_api(&self.store) {
            Ok(a) => a,
            Err(e) => return ToolResult::error(format!("NeboLoop connection required: {}", e)),
        };

        match api.install_agent(code).await {
            Ok(resp) => {
                if resp.status == "payment_required" {
                    return ToolResult::ok(format!(
                        "Agent requires payment. Checkout: {}",
                        resp.checkout_url.unwrap_or_default()
                    ));
                }

                let name = resp.artifact.name.clone();
                let artifact_id = resp.artifact.id.clone();

                // Fetch and persist artifact content
                if let Err(e) = crate::persist_agent_from_api(&api, &artifact_id, &name, code, &self.store).await {
                    warn!(code, error = %e, "failed to persist agent after install");
                }

                ToolResult::ok(format!("Installed agent: {}", name))
            }
            Err(e) => ToolResult::error(format!("install failed: {}", e)),
        }
    }

    async fn handle_reload(&self, input: &serde_json::Value) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("");
        if name.is_empty() {
            return ToolResult::error("'name' is required to identify the agent to reload");
        }
        let check_update = input["check_update"].as_bool().unwrap_or(false);
        let apply_update = input["apply_update"].as_bool().unwrap_or(false);

        // Find the agent in DB
        let db_agent = match self.store.list_agents(500, 0) {
            Ok(agents) => {
                let lower = name.to_lowercase();
                agents.into_iter().find(|r| r.name.to_lowercase() == lower || r.id == name)
            }
            Err(e) => return ToolResult::error(format!("Failed to query agents: {}", e)),
        };
        let db_agent = match db_agent {
            Some(r) => r,
            None => return ToolResult::error(format!("Agent '{}' not found.", name)),
        };

        let agent_id = &db_agent.id;
        let mut changes = Vec::new();
        let mut current_md = db_agent.agent_md.clone();
        let mut current_frontmatter = db_agent.frontmatter.clone();
        let mut current_name = db_agent.name.clone();
        let mut current_desc = db_agent.description.clone();

        // --- Marketplace update check ---
        if (check_update || apply_update) && db_agent.kind.is_some() {
            match crate::build_neboloop_api(&self.store) {
                Ok(api) => {
                    match api.get_skill(agent_id).await {
                        Ok(detail) => {
                            let remote_version = &detail.item.version;
                            // Get local version from manifest.json if it exists
                            let local_version = db_agent.napp_path.as_ref()
                                .and_then(|p| {
                                    let manifest_path = std::path::PathBuf::from(p).join("manifest.json");
                                    std::fs::read_to_string(manifest_path).ok()
                                })
                                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                                .and_then(|v| v["version"].as_str().map(|s| s.to_string()))
                                .unwrap_or_else(|| "unknown".to_string());

                            if remote_version != &local_version && !remote_version.is_empty() {
                                if apply_update {
                                    // Re-fetch and apply the update
                                    match crate::persist_agent_from_api(&api, agent_id, &db_agent.name, db_agent.kind.as_deref().unwrap_or(""), &self.store).await {
                                        Ok(_) => {
                                            // Re-read from DB after persist
                                            if let Ok(Some(updated)) = self.store.get_agent(agent_id) {
                                                current_md = updated.agent_md;
                                                current_frontmatter = updated.frontmatter;
                                                current_name = updated.name;
                                                current_desc = updated.description;
                                            }
                                            changes.push(format!("upgraded from {} → {}", local_version, remote_version));
                                        }
                                        Err(e) => changes.push(format!("upgrade failed: {}", e)),
                                    }
                                } else {
                                    changes.push(format!("update available: {} → {} (use apply_update: true to upgrade)", local_version, remote_version));
                                }
                            } else {
                                changes.push(format!("up to date (version {})", local_version));
                            }
                        }
                        Err(e) => changes.push(format!("failed to check for updates: {}", e)),
                    }
                }
                Err(_) => changes.push("NeboLoop not connected — cannot check for updates".to_string()),
            }

            if check_update && !apply_update {
                // Just checking, don't reload from filesystem
                return ToolResult::ok(format!("Agent '{}':\n- {}", db_agent.name, changes.join("\n- ")));
            }
        }

        // --- Filesystem reload ---
        let agent_dir = if let Some(ref napp_path) = db_agent.napp_path {
            std::path::PathBuf::from(napp_path)
        } else {
            self.user_dir.join(&db_agent.name)
        };

        if !agent_dir.exists() {
            if changes.is_empty() {
                return ToolResult::error(format!(
                    "Filesystem directory not found: {}. Cannot reload.",
                    agent_dir.display()
                ));
            }
            // Had marketplace changes but no filesystem — still report
        } else {
            // Reload AGENT.md
            let agent_md_path = agent_dir.join("AGENT.md");
            if agent_md_path.exists() {
                match std::fs::read_to_string(&agent_md_path) {
                    Ok(content) => {
                        if content != current_md {
                            current_md = content;
                            changes.push("AGENT.md reloaded".to_string());
                        }
                    }
                    Err(e) => changes.push(format!("failed to read AGENT.md: {}", e)),
                }
            }

            // Reload agent.json
            let agent_json_path = agent_dir.join("agent.json");
            if agent_json_path.exists() {
                match std::fs::read_to_string(&agent_json_path) {
                    Ok(content) => {
                        if content.trim() != current_frontmatter.trim() {
                            match napp::agent::parse_agent_config(&content) {
                                Ok(config) => {
                                    current_frontmatter = content;

                                    let cron_prefix = format!("agent-{}-", agent_id);
                                    let _ = self.store.delete_cron_jobs_by_prefix(&cron_prefix);
                                    let _ = self.store.delete_agent_workflows(agent_id);
                                    self.register_config_triggers(agent_id, &config);

                                    changes.push(format!(
                                        "agent.json reloaded ({} workflows, {} inputs)",
                                        config.workflows.len(),
                                        config.inputs.len()
                                    ));
                                }
                                Err(e) => changes.push(format!("agent.json invalid, skipped: {}", e)),
                            }
                        }
                    }
                    Err(e) => changes.push(format!("failed to read agent.json: {}", e)),
                }
            }
        }

        if changes.is_empty() {
            return ToolResult::ok(format!("Agent '{}' is already in sync.", db_agent.name));
        }

        // Persist to DB
        if let Err(e) = self.store.update_agent(
            agent_id, &current_name, &current_desc, &current_md,
            &current_frontmatter, db_agent.pricing_model.as_deref(), db_agent.pricing_cost,
        ) {
            return ToolResult::error(format!("Failed to update DB: {}", e));
        }

        // Update live registry
        let mut registry = self.agent_registry.write().await;
        if let Some(active) = registry.get_mut(agent_id) {
            active.name = current_name.clone();
            active.agent_md = current_md;
            active.config = napp::agent::parse_agent_config(&current_frontmatter).ok();
            changes.push("live agent updated".to_string());
        }

        ToolResult::ok(format!("Agent '{}':\n- {}", current_name, changes.join("\n- ")))
    }

    async fn handle_repair(&self, input: &serde_json::Value) -> ToolResult {
        let name_filter = input["name"].as_str().unwrap_or("");
        let mut fixes = Vec::new();

        // 1. Fix cron expressions in agent_workflows table
        let agents = self.store.list_agents(500, 0).unwrap_or_default();
        let target_agents: Vec<&db::models::Agent> = if name_filter.is_empty() {
            agents.iter().collect()
        } else {
            let lower = name_filter.to_lowercase();
            agents.iter().filter(|r| r.name.to_lowercase() == lower || r.id == name_filter).collect()
        };

        if target_agents.is_empty() && !name_filter.is_empty() {
            return ToolResult::error(format!("Agent '{}' not found.", name_filter));
        }

        for agent in &target_agents {
            let bindings = self.store.list_agent_workflows(&agent.id).unwrap_or_default();
            for binding in &bindings {
                if binding.trigger_type != "schedule" {
                    continue;
                }
                let normalized = Self::normalize_cron(&binding.trigger_config);
                if normalized != binding.trigger_config {
                    // Update agent_workflows
                    if let Err(e) = self.store.upsert_agent_workflow(
                        &agent.id,
                        &binding.binding_name,
                        "schedule",
                        &normalized,
                        binding.description.as_deref(),
                        None,
                        None,
                        None,
                    ) {
                        fixes.push(format!("FAILED {}/{}: {} ({})", agent.name, binding.binding_name, normalized, e));
                        continue;
                    }

                    // Update cron_jobs
                    let cron_name = format!("agent-{}-{}", agent.id, binding.binding_name);
                    let command = format!("agent:{}:{}", agent.id, binding.binding_name);
                    let _ = self.store.delete_cron_job_by_name(&cron_name);
                    let _ = self.store.upsert_cron_job(
                        &cron_name, &normalized, &command, "agent_workflow", None, None, None, true,
                    );

                    fixes.push(format!("fixed {}/{}: '{}' → '{}'", agent.name, binding.binding_name, binding.trigger_config, normalized));
                }
            }

            // 2. Fix cron in frontmatter (agent.json stored in DB)
            if !agent.frontmatter.is_empty() && agent.frontmatter != "{}" {
                if let Ok(mut config) = napp::agent::parse_agent_config(&agent.frontmatter) {
                    let mut frontmatter_changed = false;
                    let mut updated_workflows = config.workflows.clone();

                    for (wf_name, binding) in &config.workflows {
                        if let napp::agent::AgentTrigger::Schedule { cron } = &binding.trigger {
                            let normalized = Self::normalize_cron(cron);
                            if normalized != *cron {
                                let mut updated = binding.clone();
                                updated.trigger = napp::agent::AgentTrigger::Schedule { cron: normalized.clone() };
                                updated_workflows.insert(wf_name.clone(), updated);
                                frontmatter_changed = true;
                                fixes.push(format!("fixed {}/{} frontmatter: '{}' → '{}'", agent.name, wf_name, cron, normalized));
                            }
                        }
                    }

                    if frontmatter_changed {
                        config.workflows = updated_workflows;
                        if let Ok(new_fm) = serde_json::to_string(&config) {
                            let _ = self.store.update_agent(
                                &agent.id, &agent.name, &agent.description, &agent.agent_md,
                                &new_fm, agent.pricing_model.as_deref(), agent.pricing_cost,
                            );

                            // Also update agent.json on disk
                            let agent_dir = self.user_dir.join(&agent.name);
                            if agent_dir.join("agent.json").exists() {
                                let _ = std::fs::write(agent_dir.join("agent.json"), &new_fm);
                            }
                        }
                    }
                }
            }

            // 3. Update live registry if active
            let mut registry = self.agent_registry.write().await;
            if let Some(active) = registry.get_mut(&agent.id) {
                if !agent.frontmatter.is_empty() {
                    active.config = napp::agent::parse_agent_config(&agent.frontmatter).ok();
                }
            }
        }

        // 4. Clean up orphan cron_jobs that reference deleted agents
        let cron_jobs = self.store.list_cron_jobs(1000, 0).unwrap_or_default();
        let all_agent_ids: Vec<&str> = agents.iter().map(|r| r.id.as_str()).collect();
        for job in &cron_jobs {
            if job.name.starts_with("agent-") && job.task_type == "agent_workflow" {
                // Extract agent ID from cron name: agent-{uuid}-{binding}
                // UUID format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx (36 chars)
                if let Some(rest) = job.name.strip_prefix("agent-") {
                    if rest.len() > 36 {
                        let aid = &rest[..36];
                        if !all_agent_ids.contains(&aid) {
                            let _ = self.store.delete_cron_job_by_name(&job.name);
                            fixes.push(format!("removed orphan cron job: {} (agent deleted)", job.name));
                        }
                    }
                }
            }
        }

        if fixes.is_empty() {
            let scope = if name_filter.is_empty() { "all agents" } else { name_filter };
            ToolResult::ok(format!("No repairs needed for {}.", scope))
        } else {
            ToolResult::ok(format!("Repaired {} issues:\n- {}", fixes.len(), fixes.join("\n- ")))
        }
    }

    /// Register triggers from an agent's config into the DB (cron_jobs + agent_workflows).
    fn register_config_triggers(&self, agent_id: &str, config: &napp::agent::AgentConfig) {
        for (binding_name, binding) in &config.workflows {
            let (trigger_type, trigger_config) = match &binding.trigger {
                napp::agent::AgentTrigger::Schedule { cron } => ("schedule", Self::normalize_cron(cron)),
                napp::agent::AgentTrigger::Heartbeat { interval, window } => {
                    let cfg = match window {
                        Some(w) => format!("{}|{}", interval, w),
                        None => interval.clone(),
                    };
                    ("heartbeat", cfg)
                }
                napp::agent::AgentTrigger::Event { sources } => ("event", sources.join(",")),
                napp::agent::AgentTrigger::Watch { plugin, command, restart_delay_secs } => {
                    let cfg = serde_json::json!({
                        "plugin": plugin,
                        "command": command,
                        "restart_delay_secs": restart_delay_secs
                    }).to_string();
                    ("watch", cfg)
                }
                napp::agent::AgentTrigger::Manual => ("manual", String::new()),
            };

            let inputs_json = if binding.inputs.is_empty() {
                None
            } else {
                serde_json::to_string(&binding.inputs).ok()
            };
            let desc = if binding.description.is_empty() {
                None
            } else {
                Some(binding.description.as_str())
            };

            let activities_json = if binding.activities.is_empty() {
                None
            } else {
                serde_json::to_string(&binding.activities).ok()
            };

            if let Err(e) = self.store.upsert_agent_workflow(
                agent_id,
                binding_name,
                trigger_type,
                &trigger_config,
                desc,
                inputs_json.as_deref(),
                binding.emit.as_deref(),
                activities_json.as_deref(),
            ) {
                warn!(agent = agent_id, binding = %binding_name, error = %e, "failed to upsert agent workflow");
            }
        }

        // Register schedule triggers as cron jobs
        if let Ok(bindings) = self.store.list_agent_workflows(agent_id) {
            for binding in &bindings {
                if binding.trigger_type == "schedule" {
                    let cron_name = format!("agent-{}-{}", agent_id, binding.binding_name);
                    let command = format!("agent:{}:{}", agent_id, binding.binding_name);
                    if let Err(e) = self.store.upsert_cron_job(
                        &cron_name,
                        &binding.trigger_config,
                        &command,
                        "agent_workflow",
                        None,
                        None,
                        None,
                        true,
                    ) {
                        warn!(agent = agent_id, binding = %binding.binding_name, error = %e, "failed to register schedule trigger");
                    }
                }
            }
        }
    }

    /// Convert structured `automations` array into an AgentConfig-compatible agent.json value.
    ///
    /// Each automation entry maps to a WorkflowBinding:
    /// - `name` → binding key
    /// - `trigger` ("schedule"|"heartbeat"|"event"|"manual") + trigger-specific fields
    /// - `steps` string array → AgentActivity objects with auto-generated IDs
    /// - `emit` → emit field on the binding
    /// - `description` → binding description
    /// Normalize a cron expression to the 7-field format required by the `cron` crate.
    ///
    /// The `cron` crate v0.12 expects: `sec min hour dom month dow year`
    /// LLMs commonly produce:
    ///   - Standard 5-field: `min hour dom month dow` (e.g. "0 7 * * *")
    ///   - Time notation: `H:MM` in the hour field (e.g. "0 9:30 * * 1-5")
    ///   - Human-readable: "every 30 seconds", "every 2 minutes", "daily at 7am"
    ///
    /// This function handles all these cases.
    pub fn normalize_cron(expr: &str) -> String {
        let trimmed = expr.trim();

        // Handle human-readable expressions like "every 30 seconds", "every 2 minutes", etc.
        let lower = trimmed.to_lowercase();
        if lower.starts_with("every ") || lower.starts_with("at ") || lower.contains("daily") || lower.contains("weekly") || lower.contains("hourly") {
            return Self::human_to_cron(&lower);
        }

        // Pre-process: fix H:MM or HH:MM notation in fields (e.g. "0 9:30 * * 1-5")
        let processed = Self::fix_time_notation(trimmed);
        let fields: Vec<&str> = processed.split_whitespace().collect();

        match fields.len() {
            5 => format!("0 {} *", processed),       // standard 5-field → 7-field
            6 => format!("0 {}", processed),          // 6-field (missing seconds) → 7-field
            7 => processed,                           // already 7-field
            _ => format!("0 {} * * * *", processed),  // best effort
        }
    }

    /// Fix H:MM or HH:MM time notation in cron fields.
    ///
    /// LLMs write "0 9:30 * * 1-5" meaning "at 9:30, weekdays".
    /// This converts the H:MM to proper minute and hour fields.
    pub fn fix_time_notation(expr: &str) -> String {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        let mut result: Vec<String> = Vec::new();
        let mut i = 0;

        while i < fields.len() {
            let field = fields[i];
            if field.contains(':') {
                // Split H:MM into separate hour and minute fields
                let parts: Vec<&str> = field.split(':').collect();
                if parts.len() == 2 {
                    let hour = parts[0];
                    let minute = parts[1];
                    // If this is the second field (index 1 in 5-field cron), the preceding
                    // field is likely "0" (minute placeholder). Replace it with the actual minute.
                    if i > 0 && result.last().map_or(false, |f| f == "0") {
                        result.pop();
                        result.push(minute.to_string());
                    } else {
                        result.push(minute.to_string());
                    }
                    result.push(hour.to_string());
                } else {
                    result.push(field.to_string());
                }
            } else {
                result.push(field.to_string());
            }
            i += 1;
        }

        result.join(" ")
    }

    /// Convert human-readable schedule expressions to 7-field cron.
    ///
    /// Handles: "every N seconds/minutes/hours", "daily at Ham/Hpm",
    ///          "hourly", "weekly", "every weekday at H:MM"
    pub fn human_to_cron(expr: &str) -> String {
        let lower = expr.trim().to_lowercase();

        // "every N seconds" → */N * * * * * *
        if lower.contains("second") {
            if let Some(n) = Self::extract_number(&lower) {
                return format!("*/{} * * * * * *", n);
            }
            return "*/30 * * * * * *".to_string(); // default: every 30s
        }

        // "every N minutes" → 0 */N * * * * *
        if lower.contains("minute") {
            if let Some(n) = Self::extract_number(&lower) {
                return format!("0 */{} * * * * *", n);
            }
            return "0 */5 * * * * *".to_string(); // default: every 5min
        }

        // "every N hours" or "hourly" → 0 0 */N * * * *
        if lower.contains("hour") {
            if let Some(n) = Self::extract_number(&lower) {
                return format!("0 0 */{} * * * *", n);
            }
            return "0 0 * * * * *".to_string(); // every hour
        }

        // "daily at H" / "daily at H:MM" / "daily at Ham/Hpm"
        if lower.contains("daily") || lower.starts_with("at ") {
            let (hour, minute) = Self::extract_time(&lower);
            return format!("0 {} {} * * * *", minute, hour);
        }

        // "weekly" → Sunday at midnight
        if lower.contains("weekly") {
            let (hour, minute) = Self::extract_time(&lower);
            return format!("0 {} {} * * 0 *", minute, hour);
        }

        // "weekday" / "weekdays" → Mon-Fri
        if lower.contains("weekday") {
            let (hour, minute) = Self::extract_time(&lower);
            return format!("0 {} {} * * 1-5 *", minute, hour);
        }

        // Fallback: daily at 9am
        "0 0 9 * * * *".to_string()
    }

    /// Extract the first number from a string.
    pub fn extract_number(s: &str) -> Option<u32> {
        s.split_whitespace()
            .find_map(|word| word.parse::<u32>().ok())
    }

    /// Extract hour and minute from a human-readable time expression.
    /// Returns (hour, minute) as strings for cron fields.
    pub fn extract_time(s: &str) -> (String, String) {
        // Look for H:MM pattern
        for word in s.split_whitespace() {
            let clean = word.trim_end_matches(|c: char| !c.is_ascii_digit());
            if clean.contains(':') {
                let parts: Vec<&str> = clean.split(':').collect();
                if parts.len() == 2 {
                    if let (Ok(mut h), Ok(m)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                        // Handle am/pm suffix
                        if word.to_lowercase().contains("pm") && h < 12 {
                            h += 12;
                        }
                        return (h.to_string(), m.to_string());
                    }
                }
            }
            // Look for Hpm / Ham pattern (e.g. "7am", "6pm")
            let is_pm = word.to_lowercase().ends_with("pm");
            let is_am = word.to_lowercase().ends_with("am");
            if is_pm || is_am {
                let num_part = word.trim_end_matches(|c: char| !c.is_ascii_digit());
                if let Ok(mut h) = num_part.parse::<u32>() {
                    if is_pm && h < 12 { h += 12; }
                    if is_am && h == 12 { h = 0; }
                    return (h.to_string(), "0".to_string());
                }
            }
        }

        // Look for bare number after "at"
        if let Some(at_pos) = s.find("at ") {
            let after_at = &s[at_pos + 3..];
            for word in after_at.split_whitespace() {
                if let Ok(h) = word.parse::<u32>() {
                    if h <= 23 {
                        return (h.to_string(), "0".to_string());
                    }
                }
            }
        }

        // Default: midnight
        ("0".to_string(), "0".to_string())
    }

    /// Convert a parsed AgentTrigger into flat (type, config) strings for DB storage.
    fn flatten_trigger(trigger: &napp::agent::AgentTrigger) -> (String, String) {
        match trigger {
            napp::agent::AgentTrigger::Schedule { cron } => ("schedule".to_string(), cron.clone()),
            napp::agent::AgentTrigger::Heartbeat { interval, window } => {
                let config = match window {
                    Some(w) => format!("{}|{}", interval, w),
                    None => interval.clone(),
                };
                ("heartbeat".to_string(), config)
            }
            napp::agent::AgentTrigger::Event { sources } => ("event".to_string(), sources.join(",")),
            napp::agent::AgentTrigger::Watch { plugin, command, restart_delay_secs } => {
                let cfg = serde_json::json!({
                    "plugin": plugin,
                    "command": command,
                    "restart_delay_secs": restart_delay_secs
                }).to_string();
                ("watch".to_string(), cfg)
            }
            napp::agent::AgentTrigger::Manual => ("manual".to_string(), String::new()),
        }
    }

    fn build_agent_json_from_automations(automations: &[serde_json::Value]) -> serde_json::Value {
        let mut workflows = serde_json::Map::new();

        for auto in automations {
            let binding_name = auto["name"].as_str().unwrap_or("default");

            // Auto-infer trigger type from fields present — don't rely on LLM
            // setting the "trigger" field correctly when context fields exist.
            let trigger_type = if auto["schedule"].is_string() {
                "schedule"
            } else if auto["interval"].is_string() {
                "heartbeat"
            } else if !auto["sources"].is_null() {
                "event"
            } else {
                auto["trigger"].as_str().unwrap_or("manual")
            };

            // Build trigger object
            let trigger = match trigger_type {
                "schedule" => {
                    let raw = auto["schedule"].as_str().unwrap_or("0 9 * * *");
                    let cron = Self::normalize_cron(raw);
                    serde_json::json!({ "type": "schedule", "cron": cron })
                }
                "heartbeat" => {
                    let interval = auto["interval"].as_str().unwrap_or("30m");
                    let mut t = serde_json::json!({ "type": "heartbeat", "interval": interval });
                    if let Some(window) = auto["window"].as_str() {
                        t["window"] = serde_json::Value::String(window.to_string());
                    }
                    t
                }
                "event" => {
                    let sources: Vec<serde_json::Value> = if let Some(arr) = auto["sources"].as_array() {
                        arr.clone()
                    } else if let Some(s) = auto["sources"].as_str() {
                        s.split(',').map(|s| serde_json::Value::String(s.trim().to_string())).collect()
                    } else {
                        vec![]
                    };
                    serde_json::json!({ "type": "event", "sources": sources })
                }
                _ => serde_json::json!({ "type": "manual" }),
            };

            // Build activities from steps array
            let activities: Vec<serde_json::Value> = if let Some(steps) = auto["steps"].as_array() {
                steps.iter().enumerate().map(|(i, step)| {
                    let intent = step.as_str().unwrap_or("Execute step");
                    serde_json::json!({
                        "id": format!("step-{}", i + 1),
                        "intent": intent
                    })
                }).collect()
            } else {
                vec![]
            };

            let mut binding = serde_json::json!({
                "trigger": trigger,
                "activities": activities
            });

            if let Some(desc) = auto["description"].as_str() {
                binding["description"] = serde_json::Value::String(desc.to_string());
            }
            if let Some(emit) = auto["emit"].as_str() {
                binding["emit"] = serde_json::Value::String(emit.to_string());
            }

            workflows.insert(binding_name.to_string(), binding);
        }

        serde_json::json!({ "workflows": workflows })
    }

    async fn handle_stats(&self, input: &serde_json::Value) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("");
        if name.is_empty() {
            return ToolResult::error("'name' is required to get agent stats");
        }

        // Resolve agent_id from DB
        let db_agent = match self.store.list_agents(500, 0) {
            Ok(agents) => {
                let lower = name.to_lowercase();
                agents.into_iter().find(|r| r.name.to_lowercase() == lower || r.id == name)
            }
            Err(e) => return ToolResult::error(format!("Failed to query agents: {}", e)),
        };
        let db_agent = match db_agent {
            Some(r) => r,
            None => return ToolResult::error(format!(
                "Agent '{}' not found. Use persona(action: \"list\") to see available agents.",
                name
            )),
        };

        let agent_id = &db_agent.id;

        let stats = match self.store.agent_workflow_stats(agent_id) {
            Ok(s) => s,
            Err(e) => return ToolResult::error(format!("Failed to query stats: {}", e)),
        };

        if stats.total_runs == 0 {
            return ToolResult::ok(format!("## Stats for {}\n\nNo workflow runs recorded yet.", db_agent.name));
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // Format duration
        let duration_str = match stats.avg_duration_secs {
            Some(secs) if secs >= 60 => format!("{}m {}s", secs / 60, secs % 60),
            Some(secs) => format!("{}s", secs),
            None => "-".to_string(),
        };

        // Format relative time
        let relative = |ts: Option<i64>| -> String {
            match ts {
                Some(t) => {
                    let diff = now - t;
                    if diff < 60 { format!("{}s ago", diff) }
                    else if diff < 3600 { format!("{}m ago", diff / 60) }
                    else if diff < 86400 { format!("{}h ago", diff / 3600) }
                    else { format!("{}d ago", diff / 86400) }
                }
                None => "-".to_string(),
            }
        };

        let mut out = format!(
            "## Stats for {}\n\n\
             Runs: {} total ({} completed, {} failed, {} cancelled, {} running)\n\
             Tokens: {} total\n\
             Avg duration: {}\n\
             Last run: {}",
            db_agent.name,
            stats.total_runs, stats.completed, stats.failed, stats.cancelled, stats.running,
            stats.total_tokens,
            duration_str,
            relative(stats.last_run_at),
        );

        if let Some(ref err) = stats.last_error {
            out.push_str(&format!("\nLast error: \"{}\"", err));
        }

        // Recent errors
        let errors = self.store.agent_recent_errors(agent_id, 5).unwrap_or_default();
        if !errors.is_empty() {
            out.push_str("\n\n### Recent Errors");
            for (i, e) in errors.iter().enumerate() {
                let activity = e.activity_id.as_deref().unwrap_or("unknown");
                out.push_str(&format!(
                    "\n{}. [{}] activity \"{}\": {}",
                    i + 1,
                    relative(Some(e.started_at)),
                    activity,
                    e.error,
                ));
            }
        }

        ToolResult::ok(out)
    }

    async fn handle_setup(&self, input: &serde_json::Value) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("");
        if name.is_empty() {
            return ToolResult::error("'name' is required to set up an agent");
        }

        let db_agent = match self.store.list_agents(500, 0) {
            Ok(agents) => {
                let lower = name.to_lowercase();
                agents.into_iter().find(|r| r.name.to_lowercase() == lower || r.id == name)
            }
            Err(e) => return ToolResult::error(format!("Failed to query agents: {}", e)),
        };
        let db_agent = match db_agent {
            Some(r) => r,
            None => return ToolResult::error(format!("Agent '{}' not found.", name)),
        };

        // Return a structured result the frontend can act on
        ToolResult::ok(format!(
            "{{\"__agentSetup\": true, \"agentId\": \"{}\", \"agentName\": \"{}\", \"agentDescription\": \"{}\"}}\n\n\
             The setup wizard for **{}** is ready. The user can configure inputs and schedules in the Configure tab.",
            db_agent.id, db_agent.name, db_agent.description, db_agent.name
        ))
    }

    /// Find an agent by name across filesystem locations and DB.
    fn find_agent(&self, name: &str) -> Option<napp::agent_loader::LoadedAgent> {
        // Check user agents first (more likely to be edited)
        let user_agents = napp::agent_loader::scan_user_agents(&self.user_dir);
        for agent in user_agents {
            if agent.agent_def.name.eq_ignore_ascii_case(name) {
                return Some(agent);
            }
        }

        // Check installed agents
        let installed = napp::agent_loader::scan_installed_agents(&self.installed_dir);
        for agent in installed {
            if agent.agent_def.name.eq_ignore_ascii_case(name) {
                return Some(agent);
            }
        }

        // Fallback: check DB (agents created via REST API or marketplace install)
        if let Ok(db_agents) = self.store.list_agents(500, 0) {
            let lower = name.to_lowercase();
            for r in db_agents {
                if r.name.to_lowercase() == lower || r.id == name {
                    let agent_def = napp::agent::AgentDef {
                        id: r.id.clone(),
                        name: r.name.clone(),
                        description: r.description.clone(),
                        body: r.agent_md.clone(),
                    };
                    let config = if !r.frontmatter.is_empty() {
                        napp::agent::parse_agent_config(&r.frontmatter).ok()
                    } else {
                        None
                    };
                    return Some(napp::agent_loader::LoadedAgent {
                        agent_def,
                        config,
                        source: napp::agent_loader::AgentSource::Installed,
                        napp_path: r.napp_path.map(std::path::PathBuf::from),
                        source_path: self.installed_dir.clone(),
                        version: None,
                    });
                }
            }
        }

        None
    }
}

impl DynTool for PersonaTool {
    fn name(&self) -> &str {
        "persona"
    }

    fn description(&self) -> String {
        "Manage agent personas — who the agent is, what workflows it follows, what skills it needs.\n\n\
         Actions:\n\
         - list: list available agents (installed + user-created)\n\
         - activate: assume a persona (injects persona, registers triggers)\n\
         - deactivate: drop a persona by name (or all agents if no name given)\n\
         - info: show agent details (workflows, skills, triggers, persona)\n\
         - create: create a new agent with structured automations (preferred) or raw agent_md/agent_json\n\
         - update: edit any aspect of an existing agent — supports granular, non-destructive edits\n\
         - delete: permanently remove an agent (DB, filesystem, registry, cron jobs)\n\
         - install: install an agent from marketplace (AGNT-XXXX-XXXX)\n\
         - setup: open the setup wizard for an agent (configure inputs and schedules)\n\
         - reload: re-read AGENT.md + agent.json from filesystem and sync to DB (use after editing files on disk)\n\
         - repair: fix invalid cron expressions, orphan cron jobs, and sync triggers (optional: name to target one agent)\n\
         - stats: show workflow run statistics for an agent (total/completed/failed runs, tokens, errors)\n\n\
         AUTOMATIONS (for create and update):\n\
         Each automation needs: name, steps[], and ONE trigger pattern.\n\
         Trigger type is AUTO-INFERRED from fields — just include the right field:\n\n\
         Schedule (cron):\n  \
           {\"name\": \"x\", \"schedule\": \"<cron-or-human>\", \"steps\": [...]}\n  \
           schedule accepts: standard 5-field cron (\"0 7 * * *\"), 7-field (\"0 0 7 * * * *\"),\n  \
           or human-readable (\"daily at 7am\", \"weekdays at 9:30am\", \"every 2 hours\").\n  \
           All formats are auto-normalized to valid 7-field cron.\n\n\
         Heartbeat (recurring interval):\n  \
           {\"name\": \"x\", \"interval\": \"15m\", \"window\": \"08:00-18:00\", \"steps\": [...]}\n  \
           interval: \"5m\", \"30m\", \"1h\", etc. window: optional time range.\n\n\
         Event (reactive):\n  \
           {\"name\": \"x\", \"sources\": [\"email.received\", \"calendar.changed\"], \"steps\": [...]}\n\n\
         Manual (on-demand):\n  \
           {\"name\": \"x\", \"trigger\": \"manual\", \"steps\": [...]}\n\n\
         Optional fields: emit (event name on completion), description (human label).\n\n\
         EXAMPLES:\n  \
         persona(action: \"create\", name: \"morning-briefing\", description: \"Daily executive briefing\",\n    \
           automations: [{\"name\": \"daily-brief\", \"schedule\": \"0 7 * * *\",\n    \
             \"steps\": [\"Gather top news headlines\", \"Check calendar for today\", \"Compose briefing\"],\n    \
             \"emit\": \"briefing.ready\", \"description\": \"7am daily briefing\"}])\n  \
         persona(action: \"create\", name: \"email-monitor\", description: \"Checks email\",\n    \
           automations: [{\"name\": \"check\", \"interval\": \"15m\", \"window\": \"08:00-18:00\",\n    \
             \"steps\": [\"Check inbox for urgent emails and flag them\"]}])\n  \
         persona(action: \"update\", name: \"morning-briefing\", description: \"Updated description\")\n  \
         persona(action: \"update\", name: \"morning-briefing\",\n    \
           add_automations: [{\"name\": \"evening-recap\", \"schedule\": \"daily at 6pm\",\n    \
             \"steps\": [\"Summarize the day\"]}])\n  \
         persona(action: \"update\", name: \"morning-briefing\", remove_automations: [\"evening-recap\"])\n  \
         persona(action: \"delete\", name: \"morning-briefing\")\n  \
         persona(action: \"repair\")  — fix all agents\n  \
         persona(action: \"repair\", name: \"trading-bot\")  — fix one agent\n  \
         persona(action: \"install\", code: \"AGNT-ABCD-1234\")\n\n\
         GRANULAR UPDATE (non-destructive — change one thing without affecting the rest):\n\n\
         Update a SINGLE automation (change only what you specify):\n  \
         persona(action: \"update\", name: \"seo-auditor\", update_automation: {\n    \
           \"name\": \"weekly-audit\", \"schedule\": \"0 8 * * 1\", \"description\": \"New label\"})\n  \
         persona(action: \"update\", name: \"seo-auditor\", update_automation: {\n    \
           \"name\": \"weekly-audit\", \"steps\": [\"Step 1\", \"Step 2\", \"Step 3\"]})\n\n\
         Toggle a single automation on/off:\n  \
         persona(action: \"update\", name: \"seo-auditor\", toggle_automation: \"weekly-audit\")\n\n\
         Set user-supplied input values (feeds into every workflow run):\n  \
         persona(action: \"update\", name: \"seo-auditor\", input_values: {\n    \
           \"site_url\": \"https://example.com\", \"report_frequency\": \"weekly\"})\n\n\
         Update input field schema (dynamic form shown on Settings tab):\n  \
         persona(action: \"update\", name: \"seo-auditor\", inputs: [\n    \
           {\"key\": \"site_url\", \"label\": \"Your website\", \"type\": \"text\", \"required\": true},\n    \
           {\"key\": \"frequency\", \"label\": \"Report frequency\", \"type\": \"select\",\n     \
             \"options\": [{\"value\": \"daily\", \"label\": \"Daily\"}, {\"value\": \"weekly\", \"label\": \"Weekly\"}]}])"
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "Action to perform",
                    "enum": ["list", "activate", "deactivate", "info", "create", "update", "delete", "install", "reload", "repair", "setup", "stats"]
                },
                "name": {
                    "type": "string",
                    "description": "Agent name (for activate, deactivate, info, create, update, delete)"
                },
                "new_name": {
                    "type": "string",
                    "description": "New name to rename the agent to (for update only)"
                },
                "description": {
                    "type": "string",
                    "description": "Agent description (for create/update — auto-generates AGENT.md if agent_md not provided)"
                },
                "automations": {
                    "type": "array",
                    "description": "Structured automations. For create: sets initial automations. For update: REPLACES ALL existing automations. Trigger type is auto-inferred from fields: schedule field→schedule, interval→heartbeat, sources→event, otherwise manual.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string", "description": "Automation binding name" },
                            "trigger": { "type": "string", "enum": ["schedule", "heartbeat", "event", "manual"], "description": "Trigger type (optional — auto-inferred from schedule/interval/sources fields)" },
                            "schedule": { "type": "string", "description": "Schedule — cron (5-field: '0 7 * * *' or 7-field: '0 0 7 * * * *') or human-readable ('every 30 seconds', 'daily at 7am', 'every 2 minutes', 'weekdays at 9:30am'). Auto-normalized." },
                            "interval": { "type": "string", "description": "Interval — presence auto-sets trigger to heartbeat (e.g. '15m', '1h')" },
                            "window": { "type": "string", "description": "Time window for heartbeat (e.g. '08:00-18:00')" },
                            "sources": { "type": "array", "items": { "type": "string" }, "description": "Event sources — presence auto-sets trigger to event" },
                            "steps": { "type": "array", "items": { "type": "string" }, "description": "Activity steps — plain language instructions executed in order" },
                            "emit": { "type": "string", "description": "Event to emit on completion (e.g. 'briefing.ready')" },
                            "description": { "type": "string", "description": "Human-readable description of this automation" }
                        },
                        "required": ["name"]
                    }
                },
                "add_automations": {
                    "type": "array",
                    "description": "Add new automations WITHOUT removing existing ones (for update only). Same format as automations.",
                    "items": { "type": "object" }
                },
                "remove_automations": {
                    "type": "array",
                    "description": "Remove specific automations by name (for update only).",
                    "items": { "type": "string" }
                },
                "update_automation": {
                    "type": "object",
                    "description": "Update a SINGLE existing automation by name without affecting others (for update only). Provide only the fields you want to change.",
                    "properties": {
                        "name": { "type": "string", "description": "Binding name to update (required)" },
                        "description": { "type": "string", "description": "New description" },
                        "steps": { "type": "array", "items": { "type": "string" }, "description": "Replace activity steps" },
                        "schedule": { "type": "string", "description": "New cron schedule (changes trigger to schedule)" },
                        "interval": { "type": "string", "description": "New interval (changes trigger to heartbeat)" },
                        "window": { "type": "string", "description": "Time window for heartbeat" },
                        "sources": { "type": "array", "items": { "type": "string" }, "description": "Event sources (changes trigger to event)" },
                        "emit": { "type": "string", "description": "Event to emit on completion" }
                    },
                    "required": ["name"]
                },
                "toggle_automation": {
                    "type": "string",
                    "description": "Toggle a single automation on/off by binding name (for update only)"
                },
                "input_values": {
                    "type": "object",
                    "description": "Set user-supplied input values for the agent (for update only). Key-value pairs matching the agent's input schema."
                },
                "inputs": {
                    "type": "array",
                    "description": "Update the input field schema (for update only). Array of field definitions with key, label, type (text/textarea/number/select/checkbox/radio), description, required, default, placeholder, options.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "key": { "type": "string" },
                            "label": { "type": "string" },
                            "type": { "type": "string", "enum": ["text", "textarea", "number", "select", "checkbox", "radio", "path", "file"] },
                            "description": { "type": "string" },
                            "required": { "type": "boolean" },
                            "default": {},
                            "placeholder": { "type": "string" },
                            "options": { "type": "array", "items": { "type": "object", "properties": { "value": { "type": "string" }, "label": { "type": "string" } } } }
                        },
                        "required": ["key", "label"]
                    }
                },
                "agent_md": {
                    "type": "string",
                    "description": "AGENT.md persona content (for create/update — optional if description is provided on create)"
                },
                "agent_json": {
                    "type": ["string", "object"],
                    "description": "Raw agent.json with workflow bindings, triggers, skills (for create — use automations instead)"
                },
                "code": {
                    "type": "string",
                    "description": "Marketplace code (for install, e.g. AGNT-ABCD-1234)"
                },
                "check_update": {
                    "type": "boolean",
                    "description": "For reload: check if a newer version is available on NeboLoop (marketplace agents only)"
                },
                "apply_update": {
                    "type": "boolean",
                    "description": "For reload: download and apply the latest version from NeboLoop (marketplace agents only)"
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
            let action = input["action"].as_str().unwrap_or("");

            match action {
                "list" => self.handle_list().await,
                "activate" => self.handle_activate(&input).await,
                "deactivate" => self.handle_deactivate(&input).await,
                "info" => self.handle_info(&input).await,
                "create" => self.handle_create(&input).await,
                "update" => self.handle_update(&input).await,
                "delete" => self.handle_delete(&input).await,
                "install" => self.handle_install(&input).await,
                "reload" => self.handle_reload(&input).await,
                "repair" => self.handle_repair(&input).await,
                "setup" => self.handle_setup(&input).await,
                "stats" => self.handle_stats(&input).await,
                _ => ToolResult::error(format!(
                    "Unknown action '{}'. Available: list, activate, deactivate, info, create, update, delete, install, reload, repair, setup, stats",
                    action
                )),
            }
        })
    }
}
