use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::warn;

use db::Store;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// A single active role — its own bot with isolated persona and scoped capabilities.
#[derive(Debug, Clone)]
pub struct ActiveRole {
    /// Unique role identifier (DB id or filesystem name).
    pub role_id: String,
    /// Human-readable display name.
    pub name: String,
    /// Full ROLE.md body — becomes the system prompt identity.
    pub role_md: String,
    /// Parsed role.json config (workflows, skills, triggers).
    pub config: Option<napp::role::RoleConfig>,
    /// Optional bound NeboLoop channel.
    pub channel_id: Option<String>,
}

/// Registry of all currently active roles. Multiple roles run concurrently.
/// Key = role_id (lowercase name or DB id).
pub type RoleRegistry = Arc<RwLock<HashMap<String, ActiveRole>>>;

/// Legacy alias — callers that only need the old behavior can still compile.
pub type ActiveRoleState = RoleRegistry;

/// RoleTool manages the agent's roles — the top of the hierarchy.
/// A role defines who the agent is: persona, workflows, skills, triggers.
pub struct RoleTool {
    store: Arc<Store>,
    role_registry: RoleRegistry,
    installed_dir: PathBuf,
    user_dir: PathBuf,
}

impl RoleTool {
    pub fn new(store: Arc<Store>, role_registry: RoleRegistry) -> Self {
        let data = config::data_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            store,
            role_registry,
            installed_dir: data.join("nebo").join("roles"),
            user_dir: data.join("user").join("roles"),
        }
    }

    async fn handle_list(&self) -> ToolResult {
        // Scan filesystem
        let installed = napp::role_loader::scan_installed_roles(&self.installed_dir);
        let user = napp::role_loader::scan_user_roles(&self.user_dir);

        // Also check DB for roles
        let db_roles = self.store.list_roles(100, 0).unwrap_or_default();

        if installed.is_empty() && user.is_empty() && db_roles.is_empty() {
            return ToolResult::ok("No roles available.");
        }

        let mut lines = Vec::new();

        for role in &installed {
            lines.push(format!(
                "- [installed] {} — {}",
                role.role_def.name,
                if role.role_def.description.is_empty() { "-" } else { &role.role_def.description }
            ));
        }
        for role in &user {
            lines.push(format!(
                "- [user] {} — {}",
                role.role_def.name,
                if role.role_def.description.is_empty() { "-" } else { &role.role_def.description }
            ));
        }
        // Add DB-only roles not already in filesystem list
        let fs_names: Vec<&str> = installed.iter().chain(user.iter())
            .map(|r| r.role_def.name.as_str())
            .collect();
        for role in &db_roles {
            if !fs_names.contains(&role.name.as_str()) {
                let enabled = if role.is_enabled != 0 { "enabled" } else { "disabled" };
                lines.push(format!(
                    "- [db/{}] {} — {}",
                    enabled,
                    role.name,
                    if role.description.is_empty() { "-" } else { &role.description }
                ));
            }
        }

        let registry = self.role_registry.read().await;
        let active_count = registry.len();
        let status = if active_count > 0 {
            let names: Vec<&str> = registry.values().map(|r| r.name.as_str()).collect();
            format!(" ({} active: {})", active_count, names.join(", "))
        } else {
            String::new()
        };

        ToolResult::ok(format!("{} roles available{}:\n{}", lines.len(), status, lines.join("\n")))
    }

    async fn handle_activate(&self, input: &serde_json::Value) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("");
        if name.is_empty() {
            return ToolResult::error("'name' is required to activate a role");
        }

        // Try loading from filesystem first
        let role = self.find_role(name);

        match role {
            Some(loaded) => {
                let body = loaded.role_def.body.clone();
                let role_name = loaded.role_def.name.clone();

                // Derive a stable role_id
                let role_id = if !loaded.role_def.id.is_empty() {
                    loaded.role_def.id.clone()
                } else {
                    role_name.to_lowercase().replace(' ', "-")
                };

                // Insert into role registry (multiple roles can be active)
                let active = ActiveRole {
                    role_id: role_id.clone(),
                    name: role_name.clone(),
                    role_md: body,
                    config: loaded.config.clone(),
                    channel_id: None,
                };
                self.role_registry.write().await.insert(role_id.clone(), active);

                // Enable in DB if exists
                if let Ok(roles) = self.store.list_roles(100, 0) {
                    if let Some(db_role) = roles.iter().find(|r| r.name == role_name) {
                        if db_role.is_enabled == 0 {
                            let _ = self.store.toggle_role(&db_role.id);
                        }
                    }
                }

                let mut result = format!("Activated role: {} (id: {})", role_name, role_id);
                if let Some(ref config) = loaded.config {
                    let wf_count = config.workflows.len();
                    let skill_count = config.skills.len();
                    if wf_count > 0 || skill_count > 0 {
                        result.push_str(&format!(
                            "\nDependencies: {} workflows, {} skills",
                            wf_count, skill_count
                        ));
                    }

                    // Register triggers (cron jobs, role_workflows DB records)
                    self.register_config_triggers(&role_id, config);
                }

                ToolResult::ok(result)
            }
            None => ToolResult::error(format!(
                "Role '{}' not found. Use role(action: \"list\") to see available roles.",
                name
            )),
        }
    }

    async fn handle_deactivate(&self, input: &serde_json::Value) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("");

        let mut registry = self.role_registry.write().await;

        if name.is_empty() {
            // Deactivate all roles
            if registry.is_empty() {
                return ToolResult::ok("No roles are active.");
            }
            let names: Vec<String> = registry.values().map(|r| r.name.clone()).collect();
            registry.clear();
            ToolResult::ok(format!("Deactivated all roles: {}", names.join(", ")))
        } else {
            // Deactivate a specific role by name or id
            let lower = name.to_lowercase();
            let key = registry.iter()
                .find(|(k, v)| k.to_lowercase() == lower || v.name.to_lowercase() == lower)
                .map(|(k, _)| k.clone());
            match key {
                Some(k) => {
                    let role = registry.remove(&k).unwrap();
                    ToolResult::ok(format!("Deactivated role: {}", role.name))
                }
                None => ToolResult::error(format!(
                    "Role '{}' is not active. Active roles: {}",
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
            // Show all active roles
            let registry = self.role_registry.read().await;
            if registry.is_empty() {
                return ToolResult::ok("No roles are currently active.");
            }
            let mut lines = Vec::new();
            for (id, role) in registry.iter() {
                let preview = if role.role_md.len() > 200 {
                    format!("{}...", &role.role_md[..200])
                } else {
                    role.role_md.clone()
                };
                lines.push(format!("**{}** (id: {})\n{}", role.name, id, preview));
            }
            return ToolResult::ok(format!("Active roles ({}):\n\n{}", registry.len(), lines.join("\n\n---\n\n")));
        }

        match self.find_role(name) {
            Some(loaded) => {
                let version_str = loaded.version.as_deref().unwrap_or("-");
                let mut info = format!(
                    "Name: {}\nVersion: {}\nDescription: {}\nSource: {}\n",
                    loaded.role_def.name,
                    version_str,
                    if loaded.role_def.description.is_empty() { "-" } else { &loaded.role_def.description },
                    match loaded.source {
                        napp::role_loader::RoleSource::Installed => "marketplace",
                        napp::role_loader::RoleSource::User => "user-created",
                    },
                );

                if let Some(ref config) = loaded.config {
                    if !config.workflows.is_empty() {
                        info.push_str("\nWorkflows:\n");
                        for (binding, wf) in &config.workflows {
                            let trigger_desc = match &wf.trigger {
                                napp::role::RoleTrigger::Schedule { cron } => format!("schedule({})", cron),
                                napp::role::RoleTrigger::Heartbeat { interval, window } => {
                                    match window {
                                        Some(w) => format!("heartbeat({}, {})", interval, w),
                                        None => format!("heartbeat({})", interval),
                                    }
                                }
                                napp::role::RoleTrigger::Event { sources } => format!("event({})", sources.join(", ")),
                                napp::role::RoleTrigger::Manual => "manual".to_string(),
                            };
                            let desc = if wf.description.is_empty() { "" } else { &wf.description };
                            info.push_str(&format!("  - {} → {} [{}] {}\n", binding, wf.workflow_ref, trigger_desc, desc));
                        }
                    }
                    if !config.skills.is_empty() {
                        info.push_str(&format!("\nSkills: {}\n", config.skills.join(", ")));
                    }
                    if let Some(ref pricing) = config.pricing {
                        info.push_str(&format!("\nPricing: {} (${:.2})\n", pricing.model, pricing.cost));
                    }
                }

                // Show ROLE.md body preview
                let body = &loaded.role_def.body;
                let preview = if body.len() > 500 {
                    format!("{}...", &body[..500])
                } else {
                    body.clone()
                };
                info.push_str(&format!("\nPersona:\n{}", preview));

                ToolResult::ok(info)
            }
            None => ToolResult::error(format!("Role '{}' not found.", name)),
        }
    }

    async fn handle_create(&self, input: &serde_json::Value) -> ToolResult {
        let name = input["name"].as_str().unwrap_or("");
        let role_md = input["role_md"].as_str().unwrap_or("");

        if name.is_empty() || role_md.is_empty() {
            return ToolResult::error("'name' and 'role_md' are required to create a role");
        }

        // LLMs often send literal \n instead of real newlines in tool call strings.
        // Unescape so ROLE.md frontmatter parses correctly.
        let role_md = role_md.replace("\\n", "\n");

        let role_dir = self.user_dir.join(name);
        if role_dir.exists() {
            return ToolResult::error(format!("Role '{}' already exists at {}", name, role_dir.display()));
        }

        if let Err(e) = std::fs::create_dir_all(&role_dir) {
            return ToolResult::error(format!("Failed to create directory: {}", e));
        }

        let role_path = role_dir.join("ROLE.md");
        if let Err(e) = std::fs::write(&role_path, &role_md) {
            return ToolResult::error(format!("Failed to write ROLE.md: {}", e));
        }

        // Write role.json if provided (contains workflow bindings, triggers, skills, pricing)
        let role_json_str = input["role_json"].as_str().map(|s| s.to_string())
            .or_else(|| {
                let v = &input["role_json"];
                if v.is_object() { Some(v.to_string()) } else { None }
            });
        if let Some(ref rj) = role_json_str {
            let _ = std::fs::write(role_dir.join("role.json"), rj);
        }

        // Auto-generate manifest.json so version info is available
        let manifest_path = role_dir.join("manifest.json");
        if !manifest_path.exists() {
            let manifest = serde_json::json!({
                "name": name,
                "version": "1.0.0",
                "type": "role",
                "description": "",
            });
            let _ = std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest).unwrap_or_default());
        }

        let mut result = format!("Created role '{}' at {}", name, role_dir.display());
        if let Some(ref rj) = role_json_str {
            result.push_str(" (with role.json triggers/config)");
            // Parse and register triggers from role.json
            if let Ok(config) = napp::role::parse_role_config(rj) {
                self.register_config_triggers(name, &config);
            }
        }
        ToolResult::ok(result)
    }

    async fn handle_install(&self, input: &serde_json::Value) -> ToolResult {
        let code = input["code"].as_str().unwrap_or("");
        if code.is_empty() || !code.starts_with("ROLE-") {
            return ToolResult::error("'code' is required and must start with ROLE- (e.g. ROLE-ABCD-1234)");
        }

        // Check if already installed
        if let Ok(roles) = self.store.list_roles(100, 0) {
            if roles.iter().any(|r| r.code.as_deref() == Some(code)) {
                return ToolResult::ok(format!("Role {} is already installed.", code));
            }
        }

        let api = match crate::build_neboloop_api(&self.store) {
            Ok(a) => a,
            Err(e) => return ToolResult::error(format!("NeboLoop connection required: {}", e)),
        };

        match api.install_role(code).await {
            Ok(resp) => {
                if resp.status == "payment_required" {
                    return ToolResult::ok(format!(
                        "Role requires payment. Checkout: {}",
                        resp.checkout_url.unwrap_or_default()
                    ));
                }

                let name = resp.artifact.name.clone();
                let artifact_id = resp.artifact.id.clone();

                // Fetch and persist artifact content
                if let Err(e) = crate::persist_role_from_api(&api, &artifact_id, &name, code, &self.store).await {
                    warn!(code, error = %e, "failed to persist role after install");
                }

                ToolResult::ok(format!("Installed role: {}", name))
            }
            Err(e) => ToolResult::error(format!("install failed: {}", e)),
        }
    }

    /// Register triggers from a role's config into the DB (cron_jobs + role_workflows).
    fn register_config_triggers(&self, role_id: &str, config: &napp::role::RoleConfig) {
        for (binding_name, binding) in &config.workflows {
            let (trigger_type, trigger_config) = match &binding.trigger {
                napp::role::RoleTrigger::Schedule { cron } => ("schedule", cron.clone()),
                napp::role::RoleTrigger::Heartbeat { interval, window } => {
                    let cfg = match window {
                        Some(w) => format!("{}|{}", interval, w),
                        None => interval.clone(),
                    };
                    ("heartbeat", cfg)
                }
                napp::role::RoleTrigger::Event { sources } => ("event", sources.join(",")),
                napp::role::RoleTrigger::Manual => ("manual", String::new()),
            };

            // Try to resolve workflow_id from the ref
            let workflow_id = self.resolve_workflow_ref(&binding.workflow_ref);

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

            if let Err(e) = self.store.upsert_role_workflow(
                role_id,
                binding_name,
                &binding.workflow_ref,
                workflow_id.as_deref(),
                trigger_type,
                &trigger_config,
                desc,
                inputs_json.as_deref(),
            ) {
                warn!(role = role_id, binding = %binding_name, error = %e, "failed to upsert role workflow");
            }
        }

        // Register schedule triggers as cron jobs
        if let Ok(bindings) = self.store.list_role_workflows(role_id) {
            for binding in &bindings {
                if binding.trigger_type == "schedule" {
                    let cron_name = format!("role-{}-{}", role_id, binding.binding_name);
                    if let Some(ref workflow_id) = binding.workflow_id {
                        if let Err(e) = self.store.upsert_cron_job(
                            &cron_name,
                            &binding.trigger_config,
                            workflow_id,
                            "workflow",
                            None,
                            None,
                            None,
                            true,
                        ) {
                            warn!(role = role_id, binding = %binding.binding_name, error = %e, "failed to register schedule trigger");
                        }
                    }
                }
            }
        }
    }

    /// Resolve a workflow ref to its DB ID.
    fn resolve_workflow_ref(&self, workflow_ref: &str) -> Option<String> {
        if workflow_ref.is_empty() {
            return None;
        }
        if workflow_ref.starts_with("WORK-") {
            return self.store.get_workflow_by_code(workflow_ref)
                .ok().flatten().map(|wf| wf.id);
        }
        if let Ok(workflows) = self.store.list_workflows(100, 0) {
            let lower = workflow_ref.to_lowercase();
            for wf in &workflows {
                if wf.name.to_lowercase() == lower || wf.id == workflow_ref {
                    return Some(wf.id.clone());
                }
            }
        }
        None
    }

    /// Find a role by name across filesystem locations and DB.
    fn find_role(&self, name: &str) -> Option<napp::role_loader::LoadedRole> {
        // Check user roles first (more likely to be edited)
        let user_roles = napp::role_loader::scan_user_roles(&self.user_dir);
        for role in user_roles {
            if role.role_def.name.eq_ignore_ascii_case(name) {
                return Some(role);
            }
        }

        // Check installed roles
        let installed = napp::role_loader::scan_installed_roles(&self.installed_dir);
        for role in installed {
            if role.role_def.name.eq_ignore_ascii_case(name) {
                return Some(role);
            }
        }

        // Fallback: check DB (roles created via REST API or marketplace install)
        if let Ok(db_roles) = self.store.list_roles(500, 0) {
            let lower = name.to_lowercase();
            for r in db_roles {
                if r.name.to_lowercase() == lower || r.id == name {
                    let role_def = napp::role::RoleDef {
                        id: r.id.clone(),
                        name: r.name.clone(),
                        description: r.description.clone(),
                        body: r.role_md.clone(),
                    };
                    let config = if !r.frontmatter.is_empty() {
                        napp::role::parse_role_config(&r.frontmatter).ok()
                    } else {
                        None
                    };
                    return Some(napp::role_loader::LoadedRole {
                        role_def,
                        config,
                        source: napp::role_loader::RoleSource::Installed,
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

impl DynTool for RoleTool {
    fn name(&self) -> &str {
        "role"
    }

    fn description(&self) -> String {
        "Manage agent roles — who the agent is, what workflows it follows, what skills it needs.\n\n\
         Actions:\n\
         - list: list available roles (installed + user-created)\n\
         - activate: assume a role (injects persona, registers triggers)\n\
         - deactivate: drop a role by name (or all roles if no name given)\n\
         - info: show role details (workflows, skills, triggers, persona)\n\
         - create: create a new user role from name + ROLE.md content (+ optional role_json for triggers)\n\
         - install: install a role from marketplace (ROLE-XXXX-XXXX)\n\n\
         Examples:\n  \
         role(action: \"list\")\n  \
         role(action: \"activate\", name: \"chief-of-staff\")\n  \
         role(action: \"info\", name: \"chief-of-staff\")\n  \
         role(action: \"deactivate\")\n  \
         role(action: \"create\", name: \"my-role\", role_md: \"# My Role\\nYou are...\", role_json: {\"workflows\": {\"daily\": {\"ref\": \"WORK-XXXX\", \"trigger\": {\"schedule\": {\"cron\": \"0 9 * * *\"}}}}})\n  \
         role(action: \"install\", code: \"ROLE-ABCD-1234\")"
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "Action to perform",
                    "enum": ["list", "activate", "deactivate", "info", "create", "install"]
                },
                "name": {
                    "type": "string",
                    "description": "Role name (for activate, deactivate, info, create)"
                },
                "role_md": {
                    "type": "string",
                    "description": "ROLE.md content (for create)"
                },
                "role_json": {
                    "type": ["string", "object"],
                    "description": "role.json content with workflow bindings, triggers, skills (for create)"
                },
                "code": {
                    "type": "string",
                    "description": "Marketplace code (for install, e.g. ROLE-ABCD-1234)"
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
                "install" => self.handle_install(&input).await,
                _ => ToolResult::error(format!(
                    "Unknown action '{}'. Available: list, activate, deactivate, info, create, install",
                    action
                )),
            }
        })
    }
}
