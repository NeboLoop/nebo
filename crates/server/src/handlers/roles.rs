use axum::extract::{Path, Query, State};
use axum::response::Json;
use axum::http::StatusCode;
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
    // Blank role: create a minimal agent and auto-activate it
    if body.get("blank").and_then(|v| v.as_bool()).unwrap_or(false) {
        return create_blank_role(state).await;
    }

    let role_md = body["roleMd"]
        .as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("roleMd required".into())))?;

    let (fm, _body) = parse_role_md(role_md).map_err(to_error_response)?;

    let id = uuid::Uuid::new_v4().to_string();
    let kind = body["kind"].as_str().or_else(|| body["code"].as_str());
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

    // Merge roleJson skills into frontmatter so they persist on query
    let mut merged_skills = fm.skills.clone();

    if let Some(role_json_str) = extract_role_json_str(&body) {
        if let Ok(role_config) = napp::role::parse_role_config(&role_json_str) {
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
                "workflows": {},
                "skills": merged_skills,
                "pricing": fm.pricing.as_ref().map(|p| serde_json::json!({
                    "model": p.model,
                    "cost": p.cost,
                })),
            })
        }
    } else {
        serde_json::json!({
            "workflows": {},
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
            kind,
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

    // Cascade: resolve skill dependencies
    let mut deps = Vec::new();
    for s in &fm.skills {
        deps.push(crate::deps::DepRef {
            dep_type: crate::deps::DepType::Skill,
            reference: s.clone(),
        });
    }
    // Also pull skill deps from role.json if provided
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

    // Body fields take priority over frontmatter (allows renaming without editing ROLE.md)
    let name = body["name"].as_str().unwrap_or_else(|| {
        if fm.name.is_empty() { &existing.name } else { &fm.name }
    });
    let description = body["description"].as_str().unwrap_or_else(|| {
        if fm.description.is_empty() { &existing.description } else { &fm.description }
    });

    // Update role_md frontmatter if name/description changed via body (not via roleMd)
    let final_role_md = if body.get("roleMd").is_none() && (name != fm.name.as_str() || description != fm.description.as_str()) {
        // Rebuild role_md with updated name/description in frontmatter
        let mut new_md = String::from("---\n");
        new_md.push_str(&format!("name: \"{}\"\n", name));
        new_md.push_str(&format!("description: \"{}\"\n", description));
        // Re-serialize other frontmatter fields
        if !fm.skills.is_empty() {
            new_md.push_str(&format!("skills:\n"));
            for s in &fm.skills {
                new_md.push_str(&format!("  - \"{}\"\n", s));
            }
        }
        if let Some(ref p) = fm.pricing {
            new_md.push_str(&format!("pricing:\n  model: \"{}\"\n  cost: {}\n", p.model, p.cost));
        }
        new_md.push_str("---\n");
        if !_body.is_empty() {
            new_md.push('\n');
            new_md.push_str(&_body);
            new_md.push('\n');
        }
        new_md
    } else {
        role_md.to_string()
    };

    // Preserve existing frontmatter workflows (inline definitions) if not overridden
    let existing_fm: serde_json::Value = serde_json::from_str(&existing.frontmatter).unwrap_or_default();
    let workflows = existing_fm.get("workflows").cloned().unwrap_or(serde_json::json!({}));

    let frontmatter_json = serde_json::json!({
        "workflows": workflows,
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
            &final_role_md,
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

    // Sync in-memory role_registry if this role is active
    {
        let mut registry = state.role_registry.write().await;
        if let Some(active) = registry.get_mut(&id) {
            active.name = updated.name.clone();
            active.role_md = updated.role_md.clone();
        }
    }

    state.hub.broadcast(
        "role_updated",
        serde_json::json!({ "roleId": id, "name": updated.name, "description": updated.description }),
    );

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

    // Start or stop the role worker based on the new enabled state
    if role.is_enabled != 0 {
        state.role_workers.start_role(&id, &role.name).await;
    } else {
        state.role_workers.stop_role(&id).await;
    }

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

    // Collect skill deps from frontmatter
    let all_deps = crate::deps::extract_role_deps_from_frontmatter(&role.frontmatter);

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

        if let Err(e) = state.store.upsert_role_workflow(
            role_id,
            binding_name,
            trigger_type,
            &trigger_config,
            desc,
            inputs_json.as_deref(),
            binding.emit.as_deref(),
            activities_json.as_deref(),
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
            "triggerType": trigger_type,
            "hasActivities": binding.has_activities(),
            "status": "ok",
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
                // Look up the WorkflowBinding from config to get inline def
                let def_json = config
                    .workflows
                    .get(&b.binding_name)
                    .filter(|wb| wb.has_activities())
                    .map(|wb| wb.to_workflow_json(&b.binding_name));

                // Build emit_source from the WorkflowBinding
                let role_name = state.store.get_role(role_id).ok().flatten()
                    .map(|r| r.name)
                    .unwrap_or_else(|| role_id.to_string());
                let emit_src = config
                    .workflows
                    .get(&b.binding_name)
                    .and_then(|wb| wb.emit.as_ref())
                    .map(|emit_name| {
                        let slug = role_name.to_lowercase().replace(' ', "-");
                        format!("{}.{}", slug, emit_name)
                    });

                b.trigger_config.split(',').map(move |source| {
                    workflow::events::EventSubscription {
                        pattern: source.trim().to_string(),
                        default_inputs: b
                            .inputs
                            .as_ref()
                            .and_then(|s| serde_json::from_str(s).ok())
                            .unwrap_or_default(),
                        role_source: role_id.to_string(),
                        binding_name: b.binding_name.clone(),
                        definition_json: def_json.clone(),
                        emit_source: emit_src.clone(),
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

/// Create a blank role instance, auto-activate it, and return it.
async fn create_blank_role(state: AppState) -> HandlerResult<serde_json::Value> {
    let id = uuid::Uuid::new_v4().to_string();
    let role_md = "---\nname: New Agent\ndescription: \"\"\n---\n";

    let role = state
        .store
        .create_role(&id, None, "New Agent", "", role_md, "{}", None, None)
        .map_err(to_error_response)?;

    // Auto-activate: insert into role_registry so it shows in sidebar
    let active = tools::ActiveRole {
        role_id: id.clone(),
        name: role.name.clone(),
        role_md: role.role_md.clone(),
        config: None,
        channel_id: None,
    };
    state.role_registry.write().await.insert(id.clone(), active);
    state.role_workers.start_role(&id, &role.name).await;

    state.hub.broadcast(
        "role_installed",
        serde_json::json!({ "roleId": &id, "name": &role.name }),
    );
    state.hub.broadcast(
        "role_activated",
        serde_json::json!({ "roleId": &id, "name": &role.name }),
    );

    Ok(Json(serde_json::json!({
        "role": { "id": id, "name": role.name },
        "activated": true,
    })))
}

/// GET /roles/event-sources — returns available emit names from active workflow bindings.
pub async fn list_event_sources(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let emit_sources = state.store.list_emit_sources().map_err(to_error_response)?;

    let sources: Vec<serde_json::Value> = emit_sources
        .iter()
        .map(|es| {
            let slug = es.role_name.to_lowercase().replace(' ', "-");
            let value = format!("{}.{}", slug, es.emit);
            let label = format!("{} > {}", es.role_name, es.emit);
            serde_json::json!({
                "value": value,
                "label": label,
                "roleName": es.role_name,
                "bindingName": es.binding_name,
                "description": es.description,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({ "sources": sources })))
}

/// GET /roles/active — returns currently active roles from the RoleRegistry.
pub async fn list_active_roles(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let registry = state.role_registry.read().await;
    let roles: Vec<serde_json::Value> = registry.values().map(|role| {
        // Fetch description from DB if available
        let description = state.store.get_role(&role.role_id)
            .ok()
            .flatten()
            .map(|r| r.description)
            .unwrap_or_default();
        serde_json::json!({
            "roleId": role.role_id,
            "name": role.name,
            "description": description,
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

/// GET /roles/{id}/workflows — returns workflow bindings for a role.
pub async fn list_role_workflows(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    // Verify role exists
    state
        .store
        .get_role(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let workflows = state.store.list_role_workflows(&id).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({
        "workflows": workflows,
        "count": workflows.len(),
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

    // Persist enabled state so it survives restart
    state.store.set_role_enabled(&id, true).map_err(to_error_response)?;

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

    // Register agent in the owner's personal loop (non-blocking)
    {
        let st = state.clone();
        let name = role.name.clone();
        let slug = role.name.to_lowercase().replace(' ', "-");
        tokio::spawn(async move {
            if let Err(e) = crate::codes::register_agent_in_loop(&st, &name, &slug).await {
                warn!(role = %name, error = %e, "failed to register agent in loop");
            }
        });
    }

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
    // Persist disabled state so it survives restart
    if let Err(e) = state.store.set_role_enabled(&id, false) {
        warn!(role = %id, error = %e, "failed to persist role disabled state");
    }

    // Stop autonomous role worker (cancels heartbeat, event, schedule triggers)
    state.role_workers.stop_role(&id).await;

    let removed = state.role_registry.write().await.remove(&id);
    match removed {
        Some(role) => {
            // Deregister agent from the owner's personal loop (non-blocking)
            {
                let st = state.clone();
                let slug = role.name.to_lowercase().replace(' ', "-");
                tokio::spawn(async move {
                    if let Err(e) = crate::codes::deregister_agent_from_loop(&st, &slug).await {
                        warn!(agent = %slug, error = %e, "failed to deregister agent from loop");
                    }
                });
            }

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

/// POST /roles/{id}/duplicate — create a copy of a role and auto-activate it.
pub async fn duplicate_role(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let source = state
        .store
        .get_role(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let new_id = uuid::Uuid::new_v4().to_string();
    let new_name = format!("{} (Copy)", source.name);

    // Update frontmatter name in role_md
    let new_role_md = if source.role_md.contains("name:") {
        // Replace just the first name: line in the YAML frontmatter
        let mut result = String::new();
        let mut replaced = false;
        for line in source.role_md.lines() {
            if !replaced && line.trim_start().starts_with("name:") {
                result.push_str(&format!("name: \"{}\"\n", new_name));
                replaced = true;
            } else {
                result.push_str(line);
                result.push('\n');
            }
        }
        result
    } else {
        source.role_md.clone()
    };

    let role = state
        .store
        .create_role(
            &new_id,
            source.kind.as_deref(),
            &new_name,
            &source.description,
            &new_role_md,
            &source.frontmatter,
            source.pricing_model.as_deref(),
            source.pricing_cost,
        )
        .map_err(to_error_response)?;

    // Copy role_workflow bindings from source
    let source_workflows = state.store.list_role_workflows(&id).map_err(to_error_response)?;
    for wf in &source_workflows {
        let activities_str = wf.activities.as_ref().map(|v| v.to_string());
        let _ = state.store.upsert_role_workflow(
            &new_id,
            &wf.binding_name,
            &wf.trigger_type,
            &wf.trigger_config,
            wf.description.as_deref(),
            wf.inputs.as_deref(),
            wf.emit.as_deref(),
            activities_str.as_deref(),
        );
    }

    // Auto-activate
    let active = tools::ActiveRole {
        role_id: new_id.clone(),
        name: role.name.clone(),
        role_md: role.role_md.clone(),
        config: None,
        channel_id: None,
    };
    state.role_registry.write().await.insert(new_id.clone(), active);
    state.role_workers.start_role(&new_id, &role.name).await;

    state.hub.broadcast(
        "role_installed",
        serde_json::json!({ "roleId": &new_id, "name": &role.name }),
    );
    state.hub.broadcast(
        "role_activated",
        serde_json::json!({ "roleId": &new_id, "name": &role.name }),
    );

    Ok(Json(serde_json::json!({
        "role": { "id": new_id, "name": role.name },
        "activated": true,
    })))
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
        images: vec![],
    };

    crate::chat_dispatch::run_chat(&state, config, None).await;

    Ok(Json(serde_json::json!({
        "sessionId": session_key,
        "roleId": id,
        "status": "dispatched",
    })))
}

// ── Workflow Binding CRUD ─────────────────────────────────────────────────────

/// Build trigger JSON for role.json from flat (type, config) pair.
fn build_trigger_json(trigger_type: &str, trigger_config: &serde_json::Value) -> serde_json::Value {
    match trigger_type {
        "schedule" => {
            let cron = trigger_config.get("cron").and_then(|v| v.as_str()).unwrap_or("0 * * * *");
            serde_json::json!({ "type": "schedule", "cron": cron })
        }
        "heartbeat" => {
            let interval = trigger_config.get("interval").and_then(|v| v.as_str()).unwrap_or("30m");
            let mut t = serde_json::json!({ "type": "heartbeat", "interval": interval });
            if let Some(window) = trigger_config.get("window").and_then(|v| v.as_str()) {
                if !window.is_empty() {
                    t["window"] = serde_json::json!(window);
                }
            }
            t
        }
        "event" => {
            let sources: Vec<String> = if let Some(arr) = trigger_config.get("sources").and_then(|v| v.as_array()) {
                arr.iter().filter_map(|v| v.as_str().map(String::from)).collect()
            } else if let Some(s) = trigger_config.get("sources").and_then(|v| v.as_str()) {
                s.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
            } else {
                vec![]
            };
            serde_json::json!({ "type": "event", "sources": sources })
        }
        _ => serde_json::json!({ "type": "manual" }),
    }
}

/// Flatten trigger config for DB storage (flat string).
fn flatten_trigger_config(trigger_type: &str, trigger_config: &serde_json::Value) -> String {
    match trigger_type {
        "schedule" => trigger_config.get("cron").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        "heartbeat" => {
            let interval = trigger_config.get("interval").and_then(|v| v.as_str()).unwrap_or("30m");
            match trigger_config.get("window").and_then(|v| v.as_str()) {
                Some(w) if !w.is_empty() => format!("{}|{}", interval, w),
                _ => interval.to_string(),
            }
        }
        "event" => {
            if let Some(arr) = trigger_config.get("sources").and_then(|v| v.as_array()) {
                arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(",")
            } else {
                trigger_config.get("sources").and_then(|v| v.as_str()).unwrap_or("").to_string()
            }
        }
        _ => String::new(),
    }
}

/// Write updated frontmatter back to filesystem role.json if napp_path exists.
fn write_role_json_to_fs(napp_path: &Option<String>, frontmatter: &serde_json::Value) {
    if let Some(path) = napp_path {
        let role_json_path = std::path::Path::new(path).join("role.json");
        if let Ok(pretty) = serde_json::to_string_pretty(frontmatter) {
            if let Err(e) = std::fs::write(&role_json_path, &pretty) {
                warn!(path = %role_json_path.display(), error = %e, "failed to write role.json");
            }
        }
    }
}

/// Register triggers (schedule cron + event subscriptions) for a single binding.
async fn register_binding_triggers(
    role_id: &str,
    binding_name: &str,
    trigger_type: &str,
    trigger_config_flat: &str,
    frontmatter: &serde_json::Value,
    state: &AppState,
) {
    if trigger_type == "schedule" {
        let name = format!("role-{}-{}", role_id, binding_name);
        let command = format!("role:{}:{}", role_id, binding_name);
        if let Err(e) = state.store.upsert_cron_job(
            &name, trigger_config_flat, &command, "role_workflow", None, None, None, true,
        ) {
            warn!(role = role_id, binding = binding_name, error = %e, "failed to register cron job");
        }
    } else if trigger_type == "event" {
        // Build event subscriptions from the binding definition in frontmatter
        let binding_val = frontmatter
            .get("workflows")
            .and_then(|w| w.get(binding_name));

        let parsed_binding = binding_val
            .and_then(|v| serde_json::from_value::<napp::role::WorkflowBinding>(v.clone()).ok());

        let def_json = parsed_binding.as_ref()
            .filter(|wb| wb.has_activities())
            .map(|wb| wb.to_workflow_json(binding_name));

        // Build emit_source from binding emit field
        let emit_source = parsed_binding.as_ref()
            .and_then(|wb| wb.emit.as_ref())
            .map(|emit_name| {
                let role_name = frontmatter.get("name").and_then(|n| n.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| state.store.get_role(role_id).ok().flatten().map(|r| r.name))
                    .unwrap_or_else(|| role_id.to_string());
                let slug = role_name.to_lowercase().replace(' ', "-");
                format!("{}.{}", slug, emit_name)
            });

        for source in trigger_config_flat.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            state.event_dispatcher.subscribe(
                workflow::events::EventSubscription {
                    pattern: source.to_string(),
                    default_inputs: serde_json::Value::Object(Default::default()),
                    role_source: role_id.to_string(),
                    binding_name: binding_name.to_string(),
                    definition_json: def_json.clone(),
                    emit_source: emit_source.clone(),
                },
            ).await;
        }
    }
}

/// POST /roles/{id}/workflows — create a new workflow binding.
pub async fn create_role_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let role = state.store.get_role(&id).map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let binding_name = body["bindingName"].as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("bindingName required".into())))?;
    let trigger_type = body["triggerType"].as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("triggerType required".into())))?;
    let trigger_config = body.get("triggerConfig").cloned().unwrap_or(serde_json::json!({}));

    // Parse existing frontmatter
    let mut fm: serde_json::Value = serde_json::from_str(&role.frontmatter).unwrap_or(serde_json::json!({}));

    // Check for conflict
    if fm.get("workflows").and_then(|w| w.get(binding_name)).is_some() {
        return Err((
            StatusCode::CONFLICT,
            Json(types::api::ErrorResponse { error: format!("binding '{}' already exists", binding_name) }),
        ));
    }

    // Build binding for frontmatter
    let trigger_json = build_trigger_json(trigger_type, &trigger_config);
    let mut binding_val = serde_json::json!({
        "trigger": trigger_json,
    });
    if let Some(desc) = body.get("description").and_then(|v| v.as_str()) {
        binding_val["description"] = serde_json::json!(desc);
    }
    if let Some(inputs) = body.get("inputs") {
        binding_val["inputs"] = inputs.clone();
    }
    if let Some(activities) = body.get("activities") {
        binding_val["activities"] = activities.clone();
    }
    if let Some(budget) = body.get("budget") {
        binding_val["budget"] = budget.clone();
    }
    if let Some(emit) = body.get("emit").and_then(|v| v.as_str()) {
        binding_val["emit"] = serde_json::json!(emit);
    }

    // Insert into frontmatter
    if fm.get("workflows").is_none() {
        fm["workflows"] = serde_json::json!({});
    }
    fm["workflows"][binding_name] = binding_val;

    // Update role in DB
    state.store.update_role(
        &id, &role.name, &role.description, &role.role_md,
        &fm.to_string(), role.pricing_model.as_deref(), role.pricing_cost,
    ).map_err(to_error_response)?;

    // Upsert tracking row
    let trigger_config_flat = flatten_trigger_config(trigger_type, &trigger_config);
    let desc = body.get("description").and_then(|v| v.as_str());
    let inputs_json = body.get("inputs").and_then(|v| serde_json::to_string(v).ok());
    let emit_val = body.get("emit").and_then(|v| v.as_str());
    let activities_json = body.get("activities").and_then(|v| serde_json::to_string(v).ok());
    state.store.upsert_role_workflow(
        &id, binding_name, trigger_type, &trigger_config_flat,
        desc, inputs_json.as_deref(), emit_val, activities_json.as_deref(),
    ).map_err(to_error_response)?;

    // Register triggers
    register_binding_triggers(&id, binding_name, trigger_type, &trigger_config_flat, &fm, &state).await;

    // Write to filesystem
    write_role_json_to_fs(&role.napp_path, &fm);

    let workflows = state.store.list_role_workflows(&id).map_err(to_error_response)?;
    let wf = workflows.iter().find(|w| w.binding_name == binding_name);

    Ok(Json(serde_json::json!({
        "workflow": wf,
    })))
}

/// PUT /roles/{id}/workflows/{binding_name} — update an existing workflow binding.
pub async fn update_role_workflow(
    State(state): State<AppState>,
    Path((id, binding_name)): Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let role = state.store.get_role(&id).map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let mut fm: serde_json::Value = serde_json::from_str(&role.frontmatter).unwrap_or(serde_json::json!({}));

    // Verify binding exists
    let existing_binding = fm.get("workflows").and_then(|w| w.get(&binding_name)).cloned()
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    // Determine old trigger type for cleanup
    let old_trigger_type = existing_binding.get("trigger").and_then(|t| t.get("type")).and_then(|v| v.as_str()).unwrap_or("manual");

    // Build updated binding — merge provided fields over existing
    let mut updated = existing_binding.clone();

    if let Some(trigger_type) = body.get("triggerType").and_then(|v| v.as_str()) {
        let trigger_config = body.get("triggerConfig").cloned().unwrap_or(serde_json::json!({}));
        updated["trigger"] = build_trigger_json(trigger_type, &trigger_config);
    }
    if let Some(desc) = body.get("description") {
        updated["description"] = desc.clone();
    }
    if let Some(inputs) = body.get("inputs") {
        updated["inputs"] = inputs.clone();
    }
    if let Some(activities) = body.get("activities") {
        updated["activities"] = activities.clone();
    }
    if let Some(budget) = body.get("budget") {
        updated["budget"] = budget.clone();
    }
    if body.get("emit").is_some() {
        // Allow setting emit to null to clear it
        if let Some(emit) = body.get("emit").and_then(|v| v.as_str()) {
            updated["emit"] = serde_json::json!(emit);
        } else {
            updated.as_object_mut().map(|m| m.remove("emit"));
        }
    }

    fm["workflows"][&binding_name] = updated;

    // Update role in DB
    state.store.update_role(
        &id, &role.name, &role.description, &role.role_md,
        &fm.to_string(), role.pricing_model.as_deref(), role.pricing_cost,
    ).map_err(to_error_response)?;

    // Determine new trigger info
    let new_trigger_type = body.get("triggerType").and_then(|v| v.as_str()).unwrap_or(old_trigger_type);
    let trigger_config = body.get("triggerConfig").cloned().unwrap_or_else(|| {
        // Reconstruct from existing trigger
        existing_binding.get("trigger").cloned().unwrap_or(serde_json::json!({}))
    });
    let trigger_config_flat = flatten_trigger_config(new_trigger_type, &trigger_config);

    // Upsert tracking row
    let desc = fm["workflows"][&binding_name].get("description").and_then(|v| v.as_str());
    let inputs_json = fm["workflows"][&binding_name].get("inputs").and_then(|v| serde_json::to_string(v).ok());
    let emit_val = fm["workflows"][&binding_name].get("emit").and_then(|v| v.as_str());
    let activities_json = fm["workflows"][&binding_name].get("activities").and_then(|v| serde_json::to_string(v).ok());
    state.store.upsert_role_workflow(
        &id, &binding_name, new_trigger_type, &trigger_config_flat,
        desc, inputs_json.as_deref(), emit_val, activities_json.as_deref(),
    ).map_err(to_error_response)?;

    // If trigger type changed, unregister old triggers first
    if body.get("triggerType").is_some() {
        workflow::triggers::unregister_single_role_trigger(&id, &binding_name, &state.store);
        state.event_dispatcher.unsubscribe_binding(&id, &binding_name).await;
    }

    // Register new triggers
    register_binding_triggers(&id, &binding_name, new_trigger_type, &trigger_config_flat, &fm, &state).await;

    // Write to filesystem
    write_role_json_to_fs(&role.napp_path, &fm);

    let workflows = state.store.list_role_workflows(&id).map_err(to_error_response)?;
    let wf = workflows.iter().find(|w| w.binding_name == binding_name);

    Ok(Json(serde_json::json!({
        "workflow": wf,
    })))
}

/// POST /roles/{id}/workflows/{binding_name}/toggle — toggle a workflow binding on/off.
pub async fn toggle_role_workflow(
    State(state): State<AppState>,
    Path((id, binding_name)): Path<(String, String)>,
) -> HandlerResult<serde_json::Value> {
    // Verify role exists
    let role = state.store.get_role(&id).map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let is_active = state.store.toggle_role_workflow(&id, &binding_name).map_err(to_error_response)?;

    if is_active {
        // Re-register triggers
        let fm: serde_json::Value = serde_json::from_str(&role.frontmatter).unwrap_or_default();
        if let Ok(bindings) = state.store.list_role_workflows(&id) {
            if let Some(binding) = bindings.iter().find(|b| b.binding_name == binding_name) {
                register_binding_triggers(
                    &id, &binding_name, &binding.trigger_type, &binding.trigger_config, &fm, &state,
                ).await;
            }
        }
    } else {
        // Unregister triggers
        workflow::triggers::unregister_single_role_trigger(&id, &binding_name, &state.store);
        state.event_dispatcher.unsubscribe_binding(&id, &binding_name).await;
    }

    Ok(Json(serde_json::json!({
        "bindingName": binding_name,
        "isActive": is_active,
    })))
}

/// DELETE /roles/{id}/workflows/{binding_name} — delete a workflow binding.
pub async fn delete_role_workflow(
    State(state): State<AppState>,
    Path((id, binding_name)): Path<(String, String)>,
) -> HandlerResult<serde_json::Value> {
    let role = state.store.get_role(&id).map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    // Remove from frontmatter
    let mut fm: serde_json::Value = serde_json::from_str(&role.frontmatter).unwrap_or(serde_json::json!({}));
    if let Some(workflows) = fm.get_mut("workflows").and_then(|w| w.as_object_mut()) {
        workflows.remove(&binding_name);
    }

    // Update role in DB
    state.store.update_role(
        &id, &role.name, &role.description, &role.role_md,
        &fm.to_string(), role.pricing_model.as_deref(), role.pricing_cost,
    ).map_err(to_error_response)?;

    // Delete tracking row
    state.store.delete_single_role_workflow(&id, &binding_name).map_err(to_error_response)?;

    // Unregister triggers
    workflow::triggers::unregister_single_role_trigger(&id, &binding_name, &state.store);
    state.event_dispatcher.unsubscribe_binding(&id, &binding_name).await;

    // Write to filesystem
    write_role_json_to_fs(&role.napp_path, &fm);

    Ok(Json(serde_json::json!({
        "message": format!("Binding '{}' deleted", binding_name),
    })))
}
