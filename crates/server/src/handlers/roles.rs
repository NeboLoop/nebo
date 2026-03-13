use axum::extract::{Path, Query, State};
use axum::response::Json;
use serde::Deserialize;
use tracing::{info, warn};

use crate::state::AppState;
use super::{to_error_response, HandlerResult};


#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    50
}

/// Extract roleJson from request body — handles both string and object values.
fn extract_role_json_str(body: &serde_json::Value) -> Option<String> {
    let val = &body["roleJson"];
    if let Some(s) = val.as_str() {
        Some(s.to_string())
    } else if val.is_object() {
        Some(val.to_string())
    } else {
        None
    }
}

/// Parse ROLE.md content: extract YAML frontmatter between `---` delimiters.
fn parse_role_md(content: &str) -> Result<(RoleFrontmatter, String), types::NeboError> {
    let trimmed = content.trim();
    if !trimmed.starts_with("---") {
        return Err(types::NeboError::Validation(
            "ROLE.md must start with YAML frontmatter (---)".into(),
        ));
    }

    let after_first = &trimmed[3..];
    let end_pos = after_first
        .find("---")
        .ok_or_else(|| types::NeboError::Validation("missing closing --- in frontmatter".into()))?;

    let yaml_str = &after_first[..end_pos];
    let body = after_first[end_pos + 3..].trim().to_string();

    let fm: RoleFrontmatter = serde_yaml::from_str(yaml_str)
        .map_err(|e| types::NeboError::Validation(format!("invalid YAML frontmatter: {}", e)))?;

    Ok((fm, body))
}

#[derive(Debug, Deserialize)]
struct RoleFrontmatter {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    workflows: Vec<String>,
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    pricing: Option<RolePricing>,
}

#[derive(Debug, Deserialize)]
struct RolePricing {
    #[serde(default)]
    model: String,
    #[serde(default)]
    cost: f64,
}

/// GET /roles
pub async fn list_roles(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> HandlerResult<serde_json::Value> {
    let limit = q.limit.min(100);
    let db_roles = state.store.list_roles(limit, q.offset).map_err(to_error_response)?;
    let total = state.store.count_roles().unwrap_or(0);

    // Also scan filesystem roles (matching agent behavior)
    let mut fs_roles = Vec::new();
    if let Ok(data_dir) = config::data_dir() {
        let installed = napp::role_loader::scan_installed_roles(&data_dir.join("nebo").join("roles"));
        let user = napp::role_loader::scan_user_roles(&data_dir.join("user").join("roles"));

        let db_names: Vec<&str> = db_roles.iter().map(|r| r.name.as_str()).collect();
        for role in installed.into_iter().chain(user.into_iter()) {
            if !db_names.contains(&role.role_def.name.as_str()) {
                let source = match role.source {
                    napp::role_loader::RoleSource::Installed => "installed",
                    napp::role_loader::RoleSource::User => "user",
                };
                fs_roles.push(serde_json::json!({
                    "name": role.role_def.name,
                    "description": role.role_def.description,
                    "source": source,
                    "version": role.version,
                    "isEnabled": true,
                }));
            }
        }
    }

    Ok(Json(serde_json::json!({
        "roles": db_roles,
        "filesystemRoles": fs_roles,
        "total": total + fs_roles.len() as i64,
    })))
}

/// POST /roles
pub async fn create_role(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let role_md = body["roleMd"]
        .as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("roleMd required".into())))?;

    let (fm, _body) = parse_role_md(role_md).map_err(to_error_response)?;

    let id = uuid::Uuid::new_v4().to_string();
    let code = body["code"].as_str();
    let name = if fm.name.is_empty() {
        body["name"]
            .as_str()
            .ok_or_else(|| to_error_response(types::NeboError::Validation("name required in body or frontmatter".into())))?
    } else {
        &fm.name
    };
    let description = if fm.description.is_empty() {
        body["description"].as_str().unwrap_or("")
    } else {
        &fm.description
    };

    // Merge roleJson refs into frontmatter so they persist on query
    let mut merged_workflows = fm.workflows.clone();
    let mut merged_skills = fm.skills.clone();

    if let Some(role_json_str) = extract_role_json_str(&body) {
        if let Ok(role_config) = napp::role::parse_role_config(&role_json_str) {
            for (_, binding) in &role_config.workflows {
                if !merged_workflows.contains(&binding.workflow_ref) {
                    merged_workflows.push(binding.workflow_ref.clone());
                }
            }
            for s in &role_config.skills {
                if !merged_skills.contains(s) {
                    merged_skills.push(s.clone());
                }
            }
        }
    }

    // Build frontmatter: include roleJson data if present for full trigger info
    let frontmatter_json = if let Some(ref rj_str) = extract_role_json_str(&body) {
        if let Ok(_role_config) = napp::role::parse_role_config(rj_str) {
            // Store full roleJson as frontmatter so GET returns trigger data
            let mut fm_val: serde_json::Value = serde_json::from_str(rj_str).unwrap_or_default();
            // Ensure workflows/skills include merged values from ROLE.md frontmatter
            if let Some(obj) = fm_val.as_object_mut() {
                if !obj.contains_key("workflows") {
                    obj.insert("workflows".into(), serde_json::json!({}));
                }
                if !obj.contains_key("skills") {
                    obj.insert("skills".into(), serde_json::json!(merged_skills));
                }
            }
            // Add pricing from ROLE.md frontmatter if not in roleJson
            if let (Some(obj), Some(p)) = (fm_val.as_object_mut(), &fm.pricing) {
                if !obj.contains_key("pricing") {
                    obj.insert("pricing".into(), serde_json::json!({
                        "model": p.model,
                        "cost": p.cost,
                    }));
                }
            }
            fm_val
        } else {
            serde_json::json!({
                "workflows": merged_workflows,
                "skills": merged_skills,
                "pricing": fm.pricing.as_ref().map(|p| serde_json::json!({
                    "model": p.model,
                    "cost": p.cost,
                })),
            })
        }
    } else {
        serde_json::json!({
            "workflows": merged_workflows,
            "skills": merged_skills,
            "pricing": fm.pricing.as_ref().map(|p| serde_json::json!({
                "model": p.model,
                "cost": p.cost,
            })),
        })
    };

    let pricing_model = fm.pricing.as_ref().map(|p| p.model.as_str());
    let pricing_cost = fm.pricing.as_ref().map(|p| p.cost);

    let role = state
        .store
        .create_role(
            &id,
            code,
            name,
            description,
            role_md,
            &frontmatter_json.to_string(),
            pricing_model,
            pricing_cost,
        )
        .map_err(to_error_response)?;

    // Write ROLE.md and role.json to user/roles/{name}/ for filesystem-based loading
    if let Ok(user_dir) = config::user_dir() {
        let role_dir = user_dir.join("roles").join(name);
        if std::fs::create_dir_all(&role_dir).is_ok() {
            let _ = std::fs::write(role_dir.join("ROLE.md"), role_md);
            // Write the original roleJson if provided (contains triggers, workflow bindings),
            // otherwise fall back to the merged frontmatter
            let role_json_content = extract_role_json_str(&body)
                .unwrap_or_else(|| frontmatter_json.to_string());
            let _ = std::fs::write(role_dir.join("role.json"), &role_json_content);
            // Auto-generate manifest.json so version info is available
            let manifest_path = role_dir.join("manifest.json");
            if !manifest_path.exists() {
                let manifest = serde_json::json!({
                    "name": name,
                    "version": "1.0.0",
                    "type": "role",
                    "description": description,
                });
                let _ = std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest).unwrap_or_default());
            }
            let _ = state.store.set_role_napp_path(&id, &role_dir.to_string_lossy());
        }
    }

    // Process role.json workflow bindings if provided
    let mut install_report = Vec::new();
    if let Some(role_json_str) = extract_role_json_str(&body) {
        if let Ok(role_config) = napp::role::parse_role_config(&role_json_str) {
            install_report = process_role_bindings(&id, &role_config, &state).await;
        }
    }

    state.hub.broadcast(
        "role_installed",
        serde_json::json!({ "roleId": role.id, "name": role.name }),
    );

    // Cascade: resolve dependencies
    let mut deps = Vec::new();
    for w in &fm.workflows {
        deps.push(crate::deps::DepRef {
            dep_type: crate::deps::DepType::Workflow,
            reference: w.clone(),
        });
    }
    for s in &fm.skills {
        deps.push(crate::deps::DepRef {
            dep_type: crate::deps::DepType::Skill,
            reference: s.clone(),
        });
    }
    // Also pull deps from role.json if provided
    if let Some(role_json_str) = extract_role_json_str(&body) {
        if let Ok(role_config) = napp::role::parse_role_config(&role_json_str) {
            deps.extend(crate::deps::extract_role_deps(&role_config));
        }
    }

    let cascade = if !deps.is_empty() {
        let mut visited = std::collections::HashSet::new();
        Some(crate::deps::resolve_cascade(&state, deps, &mut visited).await)
    } else {
        None
    };

    Ok(Json(serde_json::json!({
        "role": role,
        "installReport": install_report,
        "cascade": cascade,
    })))
}

/// GET /roles/{id}
pub async fn get_role(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let role = state
        .store
        .get_role(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    Ok(Json(serde_json::json!({ "role": role })))
}

/// PUT /roles/{id}
pub async fn update_role(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let existing = state
        .store
        .get_role(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let role_md = body["roleMd"].as_str().unwrap_or(&existing.role_md);
    let (fm, _body) = parse_role_md(role_md).map_err(to_error_response)?;

    let name = if fm.name.is_empty() {
        body["name"].as_str().unwrap_or(&existing.name)
    } else {
        &fm.name
    };
    let description = if fm.description.is_empty() {
        body["description"].as_str().unwrap_or(&existing.description)
    } else {
        &fm.description
    };

    let frontmatter_json = serde_json::json!({
        "workflows": fm.workflows,
        "skills": fm.skills,
        "pricing": fm.pricing.as_ref().map(|p| serde_json::json!({
            "model": p.model,
            "cost": p.cost,
        })),
    });

    let pricing_model = fm.pricing.as_ref().map(|p| p.model.as_str());
    let pricing_cost = fm.pricing.as_ref().map(|p| p.cost);

    state
        .store
        .update_role(
            &id,
            name,
            description,
            role_md,
            &frontmatter_json.to_string(),
            pricing_model,
            pricing_cost,
        )
        .map_err(to_error_response)?;

    let updated = state
        .store
        .get_role(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    Ok(Json(serde_json::json!({ "role": updated })))
}

/// DELETE /roles/{id}
pub async fn delete_role(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let role = state
        .store
        .get_role(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    // Unregister triggers (cron jobs with role-{id} prefix)
    workflow::triggers::unregister_role_triggers(&id, &state.store);

    // role_workflows are cascade-deleted via FK when role is deleted
    state.store.delete_role(&id).map_err(to_error_response)?;

    // Clean up filesystem directory if it exists
    if let Some(ref napp_path) = role.napp_path {
        let path = std::path::Path::new(napp_path);
        if path.exists() {
            if let Err(e) = std::fs::remove_dir_all(path) {
                warn!(role_id = %id, path = %napp_path, error = %e, "failed to remove role directory");
            }
        }
    }

    state.hub.broadcast(
        "role_uninstalled",
        serde_json::json!({ "roleId": id, "name": role.name }),
    );

    Ok(Json(serde_json::json!({ "message": "Role deleted" })))
}

/// POST /roles/{id}/toggle
pub async fn toggle_role(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    state.store.toggle_role(&id).map_err(to_error_response)?;
    let role = state
        .store
        .get_role(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    Ok(Json(serde_json::json!({ "role": role })))
}

/// POST /roles/{id}/install-deps — attempt to resolve and install all dependencies
pub async fn install_deps(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let role = state
        .store
        .get_role(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    // Collect deps from frontmatter
    let deps = crate::deps::extract_role_deps_from_frontmatter(&role.frontmatter);

    // Also collect from role.json bindings if stored
    let mut all_deps = deps;
    if let Ok(bindings) = state.store.list_role_workflows(&id) {
        for b in &bindings {
            all_deps.push(crate::deps::DepRef {
                dep_type: crate::deps::DepType::Workflow,
                reference: b.workflow_ref.clone(),
            });
        }
    }

    // Force-install (user explicitly requested)
    let mut visited = std::collections::HashSet::new();
    let cascade = crate::deps::resolve_cascade_force(&state, all_deps, &mut visited).await;

    Ok(Json(serde_json::json!({
        "roleId": id,
        "cascade": cascade,
    })))
}

/// Process workflow bindings from role.json: upsert to DB and register triggers.
async fn process_role_bindings(
    role_id: &str,
    config: &napp::role::RoleConfig,
    state: &AppState,
) -> Vec<serde_json::Value> {
    let mut report = Vec::new();

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
        let workflow_id = resolve_workflow_ref(&binding.workflow_ref, state);

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

        if let Err(e) = state.store.upsert_role_workflow(
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
            report.push(serde_json::json!({
                "binding": binding_name,
                "status": "error",
                "error": e.to_string(),
            }));
            continue;
        }

        report.push(serde_json::json!({
            "binding": binding_name,
            "ref": binding.workflow_ref,
            "workflowId": workflow_id,
            "triggerType": trigger_type,
            "status": if workflow_id.is_some() { "linked" } else { "pending" },
        }));
    }

    // Register schedule/event triggers from the bindings
    if let Ok(bindings) = state.store.list_role_workflows(role_id) {
        workflow::triggers::register_role_triggers(role_id, &bindings, &state.store);

        // Register event subscriptions with the dispatcher
        let event_subs: Vec<_> = bindings
            .iter()
            .filter(|b| b.trigger_type == "event")
            .flat_map(|b| {
                b.trigger_config.split(',').map(move |source| {
                    workflow::events::EventSubscription {
                        pattern: source.trim().to_string(),
                        workflow_id: b.workflow_id.clone().unwrap_or_default(),
                        default_inputs: b
                            .inputs
                            .as_ref()
                            .and_then(|s| serde_json::from_str(s).ok())
                            .unwrap_or_default(),
                        role_source: role_id.to_string(),
                        binding_name: b.binding_name.clone(),
                    }
                })
            })
            .collect();

        for sub in event_subs {
            state.event_dispatcher.subscribe(sub).await;
        }
    }

    info!(role = role_id, bindings = config.workflows.len(), "processed role bindings");
    report
}

/// GET /roles/active — returns currently active roles from the RoleRegistry.
pub async fn list_active_roles(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let registry = state.role_registry.read().await;
    let roles: Vec<serde_json::Value> = registry.values().map(|role| {
        serde_json::json!({
            "roleId": role.role_id,
            "name": role.name,
            "channelId": role.channel_id,
            "hasConfig": role.config.is_some(),
            "workflowCount": role.config.as_ref().map(|c| c.workflows.len()).unwrap_or(0),
            "skillCount": role.config.as_ref().map(|c| c.skills.len()).unwrap_or(0),
        })
    }).collect();

    Ok(Json(serde_json::json!({
        "roles": roles,
        "count": roles.len(),
    })))
}

/// POST /roles/{id}/activate — activate a role from the REST API (makes it appear in sidebar).
pub async fn activate_role(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let role = state
        .store
        .get_role(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let role_id = role.id.clone();
    let config = if !role.frontmatter.is_empty() {
        napp::role::parse_role_config(&role.frontmatter).ok()
    } else {
        None
    };

    let active = tools::ActiveRole {
        role_id: role_id.clone(),
        name: role.name.clone(),
        role_md: role.role_md.clone(),
        config,
        channel_id: None,
    };

    state.role_registry.write().await.insert(role_id.clone(), active);

    // Start autonomous role worker (heartbeat, event, schedule triggers)
    state.role_workers.start_role(&role_id, &role.name).await;

    state.hub.broadcast(
        "role_activated",
        serde_json::json!({ "roleId": role_id, "name": role.name }),
    );

    Ok(Json(serde_json::json!({
        "roleId": role_id,
        "name": role.name,
        "status": "active",
    })))
}

/// POST /roles/{id}/deactivate — deactivate a role from the REST API.
pub async fn deactivate_role(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    // Stop autonomous role worker (cancels heartbeat, event, schedule triggers)
    state.role_workers.stop_role(&id).await;

    let removed = state.role_registry.write().await.remove(&id);
    match removed {
        Some(role) => {
            state.hub.broadcast(
                "role_deactivated",
                serde_json::json!({ "roleId": id, "name": role.name }),
            );
            Ok(Json(serde_json::json!({
                "roleId": id,
                "name": role.name,
                "status": "deactivated",
            })))
        }
        None => Err(to_error_response(types::NeboError::NotFound)),
    }
}

/// POST /roles/{id}/chat — send a message to a role's agent via the unified chat pipeline.
pub async fn chat_with_role(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let prompt = body["prompt"]
        .as_str()
        .unwrap_or("")
        .to_string();
    if prompt.is_empty() {
        return Err(to_error_response(types::NeboError::Validation(
            "prompt is required".into(),
        )));
    }

    // Verify role is active
    {
        let reg = state.role_registry.read().await;
        if !reg.contains_key(&id) {
            return Err(to_error_response(types::NeboError::Validation(
                format!("Role '{}' is not active. Activate it first.", id),
            )));
        }
    }

    let session_key = agent::keyparser::build_role_session_key(&id, "web");

    let entity_config = crate::entity_config::resolve_for_chat(&state.store, "role", &id);

    let config = crate::chat_dispatch::ChatConfig {
        session_key: session_key.clone(),
        prompt,
        system: String::new(),
        user_id: String::new(),
        channel: "web".to_string(),
        origin: tools::Origin::User,
        role_id: id.clone(),
        cancel_token: tokio_util::sync::CancellationToken::new(),
        lane: types::constants::lanes::MAIN.to_string(),
        comm_reply: None,
        entity_config,
    };

    crate::chat_dispatch::run_chat(&state, config, None).await;

    Ok(Json(serde_json::json!({
        "sessionId": session_key,
        "roleId": id,
        "status": "dispatched",
    })))
}

/// Try to resolve a workflow ref to a local workflow ID.
fn resolve_workflow_ref(workflow_ref: &str, state: &AppState) -> Option<String> {
    // Try by code (WORK-XXXX-XXXX)
    if workflow_ref.starts_with("WORK-") {
        return state
            .store
            .get_workflow_by_code(workflow_ref)
            .ok()
            .flatten()
            .map(|wf| wf.id);
    }
    // Try by name search for qualified refs
    if let Ok(workflows) = state.store.list_workflows(100, 0) {
        let lower = workflow_ref.to_lowercase();
        for wf in &workflows {
            if wf.name.to_lowercase() == lower || wf.id == workflow_ref {
                return Some(wf.id.clone());
            }
        }
    }
    None
}

