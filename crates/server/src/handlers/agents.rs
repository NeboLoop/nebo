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

/// Extract agentJson from request body — handles both string and object values.
fn extract_agent_json_str(body: &serde_json::Value) -> Option<String> {
    let val = &body["agentJson"];
    if let Some(s) = val.as_str() {
        Some(s.to_string())
    } else if val.is_object() {
        Some(val.to_string())
    } else {
        None
    }
}

/// Parse AGENT.md content: extract YAML frontmatter between `---` delimiters.
/// If content starts with `---` but has no closing delimiter, treats as pure prose.
fn parse_agent_md(content: &str) -> Result<(AgentFrontmatter, String), types::NeboError> {
    let (yaml_str, body) = napp::agent::split_frontmatter(content)
        .map_err(|e| types::NeboError::Validation(e.to_string()))?;
    if yaml_str.is_empty() {
        return Ok((AgentFrontmatter::default(), body));
    }

    let fm: AgentFrontmatter = serde_yaml::from_str(&yaml_str)
        .map_err(|e| types::NeboError::Validation(format!("invalid YAML frontmatter: {}", e)))?;

    Ok((fm, body))
}

/// Parse frontmatter YAML into a list of {key, value} objects for the frontend.
/// Value is either a string or an array of strings.
fn parse_persona_properties(yaml_str: &str) -> Vec<serde_json::Value> {
    if yaml_str.is_empty() {
        return vec![];
    }
    // Parse as generic YAML mapping
    let mapping: serde_yaml::Mapping = match serde_yaml::from_str(yaml_str) {
        Ok(m) => m,
        Err(_) => return vec![],
    };
    mapping.into_iter().filter_map(|(k, v)| {
        let key = match k {
            serde_yaml::Value::String(s) => s,
            _ => return None,
        };
        let value = match v {
            serde_yaml::Value::String(s) => serde_json::json!(s),
            serde_yaml::Value::Number(n) => serde_json::json!(n.to_string()),
            serde_yaml::Value::Bool(b) => serde_json::json!(b.to_string()),
            serde_yaml::Value::Sequence(seq) => {
                let items: Vec<String> = seq.into_iter().filter_map(|item| match item {
                    serde_yaml::Value::String(s) => Some(s),
                    other => Some(format!("{:?}", other)),
                }).collect();
                serde_json::json!(items)
            }
            serde_yaml::Value::Mapping(m) => {
                // Flatten nested mapping to a compact YAML string for display
                match serde_yaml::to_string(&serde_yaml::Value::Mapping(m)) {
                    Ok(s) => serde_json::json!(s.trim().to_string()),
                    Err(_) => serde_json::json!(""),
                }
            }
            _ => serde_json::json!(""),
        };
        Some(serde_json::json!({ "key": key, "value": value }))
    }).collect()
}

#[derive(Debug, Default, Deserialize)]
struct AgentFrontmatter {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    pricing: Option<AgentPricing>,
}

#[derive(Debug, Deserialize)]
struct AgentPricing {
    #[serde(default)]
    model: String,
    #[serde(default)]
    cost: f64,
}

/// GET /agents
pub async fn list_agents(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> HandlerResult<serde_json::Value> {
    let limit = q.limit.min(100);
    let db_agents = state.store.list_agents(limit, q.offset).map_err(to_error_response)?;
    let total = state.store.count_agents().unwrap_or(0);

    // Also scan filesystem agents (matching agent behavior)
    let mut fs_agents = Vec::new();
    if let Ok(data_dir) = config::data_dir() {
        let installed = napp::agent_loader::scan_installed_agents(&data_dir.join("nebo").join("agents"));
        let user = napp::agent_loader::scan_user_agents(&data_dir.join("user").join("agents"));

        let db_names: Vec<&str> = db_agents.iter().map(|r| r.name.as_str()).collect();
        for agent in installed.into_iter().chain(user.into_iter()) {
            if !db_names.contains(&agent.agent_def.name.as_str()) {
                let source = match agent.source {
                    napp::agent_loader::AgentSource::Installed => "installed",
                    napp::agent_loader::AgentSource::User => "user",
                };
                fs_agents.push(serde_json::json!({
                    "name": agent.agent_def.name,
                    "description": agent.agent_def.description,
                    "source": source,
                    "version": agent.version,
                    "isEnabled": true,
                }));
            }
        }
    }

    Ok(Json(serde_json::json!({
        "agents": db_agents,
        "filesystemAgents": fs_agents,
        "total": total + fs_agents.len() as i64,
    })))
}

/// POST /agents
pub async fn create_agent(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    // Blank agent: create a minimal agent and auto-activate it
    if body.get("blank").and_then(|v| v.as_bool()).unwrap_or(false) {
        return create_blank_agent(state).await;
    }

    let agent_md = body["agentMd"]
        .as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("agentMd required".into())))?;

    let (fm, _body) = parse_agent_md(agent_md).map_err(to_error_response)?;

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

    // Merge agentJson skills into frontmatter so they persist on query
    let mut merged_skills = fm.skills.clone();

    if let Some(agent_json_str) = extract_agent_json_str(&body) {
        if let Ok(agent_config) = napp::agent::parse_agent_config(&agent_json_str) {
            for s in &agent_config.skills {
                if !merged_skills.contains(s) {
                    merged_skills.push(s.clone());
                }
            }
        }
    }

    // Build frontmatter: include agentJson data if present for full trigger info
    let frontmatter_json = if let Some(ref rj_str) = extract_agent_json_str(&body) {
        if let Ok(_agent_config) = napp::agent::parse_agent_config(rj_str) {
            // Store full agentJson as frontmatter so GET returns trigger data
            let mut fm_val: serde_json::Value = serde_json::from_str(rj_str).unwrap_or_default();
            // Ensure workflows/skills include merged values from AGENT.md frontmatter
            if let Some(obj) = fm_val.as_object_mut() {
                if !obj.contains_key("workflows") {
                    obj.insert("workflows".into(), serde_json::json!({}));
                }
                if !obj.contains_key("skills") {
                    obj.insert("skills".into(), serde_json::json!(merged_skills));
                }
            }
            // Add pricing from AGENT.md frontmatter if not in agentJson
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

    let agent = state
        .store
        .create_agent(
            &id,
            kind,
            name,
            description,
            agent_md,
            &frontmatter_json.to_string(),
            pricing_model,
            pricing_cost,
        )
        .map_err(to_error_response)?;

    // Write AGENT.md and agent.json to user/agents/{name}/ for filesystem-based loading
    if let Ok(user_dir) = config::user_dir() {
        let agent_dir = user_dir.join("agents").join(name);
        if std::fs::create_dir_all(&agent_dir).is_ok() {
            let _ = std::fs::write(agent_dir.join("AGENT.md"), agent_md);
            // Write the original agentJson if provided (contains triggers, workflow bindings),
            // otherwise fall back to the merged frontmatter
            let agent_json_content = extract_agent_json_str(&body)
                .unwrap_or_else(|| frontmatter_json.to_string());
            let _ = std::fs::write(agent_dir.join("agent.json"), &agent_json_content);
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
            let _ = state.store.set_agent_napp_path(&id, &agent_dir.to_string_lossy());
        }
    }

    // Process agent.json workflow bindings if provided
    let mut install_report = Vec::new();
    if let Some(agent_json_str) = extract_agent_json_str(&body) {
        if let Ok(agent_config) = napp::agent::parse_agent_config(&agent_json_str) {
            install_report = process_agent_bindings(&id, &agent_config, &state).await;
        }
    }

    state.hub.broadcast(
        "agent_installed",
        serde_json::json!({ "agentId": agent.id, "name": agent.name }),
    );

    // Cascade: resolve skill dependencies
    let mut deps = Vec::new();
    for s in &fm.skills {
        deps.push(crate::deps::DepRef {
            dep_type: crate::deps::DepType::Skill,
            reference: s.clone(),
        });
    }
    // Also pull skill deps from agent.json if provided
    if let Some(agent_json_str) = extract_agent_json_str(&body) {
        if let Ok(agent_config) = napp::agent::parse_agent_config(&agent_json_str) {
            deps.extend(crate::deps::extract_agent_deps(&agent_config));
        }
    }

    let cascade = if !deps.is_empty() {
        let mut visited = std::collections::HashSet::new();
        Some(crate::deps::resolve_cascade(&state, deps, &mut visited).await)
    } else {
        None
    };

    Ok(Json(serde_json::json!({
        "agent": agent,
        "installReport": install_report,
        "cascade": cascade,
    })))
}

/// GET /agents/{id}
pub async fn get_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let agent = state
        .store
        .get_agent(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    // Read local version from manifest.json if it exists
    let version = agent.napp_path.as_ref()
        .and_then(|p| {
            let manifest_path = std::path::PathBuf::from(p).join("manifest.json");
            std::fs::read_to_string(manifest_path).ok()
        })
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v["version"].as_str().map(|s| s.to_string()));

    // Read views.json for deterministic UI declarations
    let views = agent.napp_path.as_ref()
        .and_then(|p| {
            let views_path = std::path::PathBuf::from(p).join("views.json");
            std::fs::read_to_string(views_path).ok()
        })
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());

    // Parse and normalize inputFields from frontmatter so frontend doesn't have to
    let input_fields: Vec<serde_json::Value> = if !agent.frontmatter.is_empty() {
        serde_json::from_str::<serde_json::Value>(&agent.frontmatter)
            .ok()
            .and_then(|fm| fm.get("inputs").and_then(|v| v.as_array().cloned()))
            .unwrap_or_default()
            .into_iter()
            .map(|f| {
                let name = f.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let key = f.get("key").and_then(|v| v.as_str()).unwrap_or(name);
                let label = f.get("label").and_then(|v| v.as_str()).map(|s| s.to_string())
                    .unwrap_or_else(|| {
                        name.replace('_', " ").replace('-', " ")
                            .split_whitespace()
                            .map(|w| {
                                let mut c = w.chars();
                                match c.next() {
                                    None => String::new(),
                                    Some(ch) => ch.to_uppercase().to_string() + c.as_str(),
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(" ")
                    });
                // Normalize options: plain strings → { value, label }
                let options = f.get("options").and_then(|v| v.as_array()).map(|arr| {
                    arr.iter().map(|o| {
                        if let Some(s) = o.as_str() {
                            serde_json::json!({ "value": s, "label": s.replace('_', " ").replace('-', " ") })
                        } else {
                            o.clone()
                        }
                    }).collect::<Vec<_>>()
                });

                let mut field = serde_json::json!({
                    "key": if key.is_empty() { name } else { key },
                    "label": label,
                    "type": f.get("type").and_then(|v| v.as_str()).unwrap_or("text"),
                    "required": f.get("required").and_then(|v| v.as_bool()).unwrap_or(false),
                });
                if let Some(desc) = f.get("description").and_then(|v| v.as_str()) {
                    field["description"] = serde_json::json!(desc);
                }
                if let Some(ph) = f.get("placeholder").and_then(|v| v.as_str()) {
                    field["placeholder"] = serde_json::json!(ph);
                }
                if let Some(def) = f.get("default") {
                    field["default"] = def.clone();
                }
                if let Some(opts) = options {
                    field["options"] = serde_json::json!(opts);
                }
                field
            })
            .collect()
    } else {
        vec![]
    };

    // Split agentMd into properties + body for the persona editor
    let (yaml_str, persona_body) = napp::agent::split_frontmatter(&agent.agent_md).unwrap_or_default();
    let persona_properties = parse_persona_properties(&yaml_str);

    // Extract model and skills from frontmatter for V2 frontend
    let frontmatter_val: serde_json::Value = if !agent.frontmatter.is_empty() {
        serde_json::from_str(&agent.frontmatter).unwrap_or_default()
    } else {
        serde_json::Value::Null
    };
    let model = frontmatter_val.get("model").and_then(|v| v.as_str()).unwrap_or("");
    let skills: Vec<&str> = frontmatter_val.get("skills")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|s| s.as_str()).collect())
        .unwrap_or_default();

    Ok(Json(serde_json::json!({
        "agent": agent,
        "version": version,
        "inputFields": input_fields,
        "personaProperties": persona_properties,
        "personaBody": persona_body,
        "persona": persona_body,
        "model": model,
        "skills": skills,
        "views": views,
    })))
}

/// PUT /agents/{id}
pub async fn update_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let existing = state
        .store
        .get_agent(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let agent_md = body["agentMd"].as_str().unwrap_or(&existing.agent_md);
    let (fm, _body) = parse_agent_md(agent_md).map_err(to_error_response)?;

    // Body fields take priority over frontmatter (allows renaming without editing AGENT.md)
    let name = body["name"].as_str().unwrap_or_else(|| {
        if fm.name.is_empty() { &existing.name } else { &fm.name }
    });
    let description = body["description"].as_str().unwrap_or_else(|| {
        if fm.description.is_empty() { &existing.description } else { &fm.description }
    });

    // Update agent_md frontmatter if name/description changed via body (not via agentMd)
    let final_agent_md = if body.get("agentMd").is_none() && (name != fm.name.as_str() || description != fm.description.as_str()) {
        // Rebuild agent_md with updated name/description in frontmatter
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
        agent_md.to_string()
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
        .update_agent(
            &id,
            name,
            description,
            &final_agent_md,
            &frontmatter_json.to_string(),
            pricing_model,
            pricing_cost,
        )
        .map_err(to_error_response)?;

    let updated = state
        .store
        .get_agent(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    // Sync in-memory agent_registry if this agent is active
    {
        let mut registry = state.agent_registry.write().await;
        if let Some(active) = registry.get_mut(&id) {
            active.name = updated.name.clone();
            active.agent_md = updated.agent_md.clone();
        }
    }

    state.hub.broadcast(
        "agent_updated",
        serde_json::json!({ "agentId": id, "name": updated.name, "description": updated.description }),
    );

    Ok(Json(serde_json::json!({ "agent": updated })))
}

/// DELETE /agents/{id}
pub async fn delete_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let agent = state
        .store
        .get_agent(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let slug = agent.name.to_lowercase().replace(' ', "-");

    // Stop agent worker (cancels heartbeat, event, schedule triggers)
    state.agent_workers.stop_agent(&id).await;

    // Remove from live registry
    state.agent_registry.write().await.remove(&id);

    // Unregister triggers (cron jobs with agent-{id} prefix)
    workflow::triggers::unregister_agent_triggers(&id, &state.store);

    // Unsubscribe event triggers from dispatcher
    state.event_dispatcher.unsubscribe_agent(&id).await;

    // agent_workflows are cascade-deleted via FK when agent is deleted
    state.store.delete_agent(&id).map_err(to_error_response)?;

    // Notify frontend immediately — cleanup runs after
    state.hub.broadcast(
        "agent_uninstalled",
        serde_json::json!({ "agentId": id, "name": agent.name }),
    );

    // Clean up agent-scoped data (chats before sessions — chats reference session names)
    let _ = state.store.delete_agent_chats(&id);
    let _ = state.store.delete_agent_sessions(&id);
    let _ = state.store.delete_agent_memories(&id);
    let _ = state.store.delete_agent_workflow_runs(&id);

    // Clean up filesystem -- check napp_path, nebo/agents/, and user/agents/
    if let Some(ref napp_path) = agent.napp_path {
        let path = std::path::Path::new(napp_path);
        if path.exists() {
            let _ = std::fs::remove_dir_all(path);
        }
    }
    if let Ok(nebo_dir) = config::nebo_dir() {
        let dir = nebo_dir.join("agents").join(&slug);
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
    if let Ok(user_dir) = config::user_dir() {
        let dir = user_dir.join("agents").join(&slug);
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    // Deregister agent from NeboLoop (non-blocking, best-effort)
    {
        let st = state.clone();
        let agent_slug = slug.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::codes::deregister_agent_from_loop(&st, &agent_slug).await {
                warn!(agent = %agent_slug, error = %e, "failed to deregister agent from loop");
            }
        });
    }

    Ok(Json(serde_json::json!({ "message": "Agent deleted" })))
}

/// POST /agents/{id}/toggle
pub async fn toggle_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    state.store.toggle_agent(&id).map_err(to_error_response)?;
    let agent = state
        .store
        .get_agent(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    // Start or stop the agent worker based on the new enabled state
    if agent.is_enabled != 0 {
        state.agent_workers.start_agent(&id, &agent.name).await;
    } else {
        state.agent_workers.stop_agent(&id).await;
    }

    Ok(Json(serde_json::json!({ "agent": agent })))
}

/// POST /agents/{id}/install-deps — attempt to resolve and install all dependencies
pub async fn install_deps(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let agent = state
        .store
        .get_agent(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    // Collect skill deps from frontmatter
    let all_deps = crate::deps::extract_agent_deps_from_frontmatter(&agent.frontmatter);

    // Force-install (user explicitly requested)
    let mut visited = std::collections::HashSet::new();
    let cascade = crate::deps::resolve_cascade_force(&state, all_deps, &mut visited).await;

    Ok(Json(serde_json::json!({
        "agentId": id,
        "cascade": cascade,
    })))
}

/// Process workflow bindings from agent.json: upsert to DB and register triggers.
pub async fn process_agent_bindings(
    agent_id: &str,
    config: &napp::agent::AgentConfig,
    state: &AppState,
) -> Vec<serde_json::Value> {
    let mut report = Vec::new();

    for (binding_name, binding) in &config.workflows {
        let (trigger_type, trigger_config) = match &binding.trigger {
            napp::agent::AgentTrigger::Schedule { cron, .. } => ("schedule", tools::PersonaTool::normalize_cron(cron)),
            napp::agent::AgentTrigger::Heartbeat { interval, window } => {
                let cfg = match window {
                    Some(w) => format!("{}|{}", interval, w),
                    None => interval.clone(),
                };
                ("heartbeat", cfg)
            }
            napp::agent::AgentTrigger::Event { sources } => ("event", sources.join(",")),
            napp::agent::AgentTrigger::Watch { plugin, command, event, restart_delay_secs } => {
                let mut cfg = serde_json::json!({
                    "plugin": plugin,
                    "command": command,
                    "restart_delay_secs": restart_delay_secs
                });
                if let Some(ev) = event {
                    cfg["event"] = serde_json::json!(ev);
                }
                ("watch", cfg.to_string())
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

        let connections_json = if binding.connections.is_empty() {
            None
        } else {
            serde_json::to_string(&binding.connections).ok()
        };

        if let Err(e) = state.store.upsert_agent_workflow(
            agent_id,
            binding_name,
            trigger_type,
            &trigger_config,
            desc,
            inputs_json.as_deref(),
            binding.emit.as_deref(),
            activities_json.as_deref(),
            connections_json.as_deref(),
        ) {
            warn!(agent = agent_id, binding = %binding_name, error = %e, "failed to upsert agent workflow");
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
    if let Ok(bindings) = state.store.list_agent_workflows(agent_id) {
        workflow::triggers::register_agent_triggers(agent_id, &bindings, &state.store);

        // Register event subscriptions with the dispatcher (only active bindings)
        let event_subs: Vec<_> = bindings
            .iter()
            .filter(|b| b.trigger_type == "event" && b.is_active == 1)
            .flat_map(|b| {
                // Look up the WorkflowBinding from config to get inline def
                let def_json = config
                    .workflows
                    .get(&b.binding_name)
                    .filter(|wb| wb.has_activities())
                    .map(|wb| wb.to_workflow_json(&b.binding_name));

                // Build emit_source from the WorkflowBinding
                let agent_name = state.store.get_agent(agent_id).ok().flatten()
                    .map(|r| r.name)
                    .unwrap_or_else(|| agent_id.to_string());
                let emit_src = config
                    .workflows
                    .get(&b.binding_name)
                    .and_then(|wb| wb.emit.as_ref())
                    .map(|emit_name| {
                        let slug = agent_name.to_lowercase().replace(' ', "-");
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
                        agent_source: agent_id.to_string(),
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

    info!(agent = agent_id, bindings = config.workflows.len(), "processed agent bindings");
    report
}

/// Create a blank agent instance, auto-activate it, and return it.
async fn create_blank_agent(state: AppState) -> HandlerResult<serde_json::Value> {
    let id = uuid::Uuid::new_v4().to_string();
    let agent_md = "---\nname: New Agent\ndescription: \"\"\n---\n";

    let agent = state
        .store
        .create_agent(&id, None, "New Agent", "", agent_md, "{}", None, None)
        .map_err(to_error_response)?;

    // Auto-activate: insert into agent_registry so it shows in sidebar
    let active = tools::ActiveAgent {
        agent_id: id.clone(),
        name: agent.name.clone(),
        agent_md: agent.agent_md.clone(),
        config: None,
        channel_id: None,
        degraded: None,
    };
    state.agent_registry.write().await.insert(id.clone(), active);
    state.agent_workers.start_agent(&id, &agent.name).await;

    state.hub.broadcast(
        "agent_installed",
        serde_json::json!({ "agentId": &id, "name": &agent.name }),
    );
    state.hub.broadcast(
        "agent_activated",
        serde_json::json!({ "agentId": &id, "name": &agent.name }),
    );

    Ok(Json(serde_json::json!({
        "agent": { "id": id, "name": agent.name },
        "activated": true,
    })))
}

/// GET /agents/event-sources — returns available emit names from active workflow bindings.
pub async fn list_event_sources(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let emit_sources = state.store.list_emit_sources().map_err(to_error_response)?;

    let sources: Vec<serde_json::Value> = emit_sources
        .iter()
        .map(|es| {
            let slug = es.agent_name.to_lowercase().replace(' ', "-");
            let value = format!("{}.{}", slug, es.emit);
            let label = format!("{} > {}", es.agent_name, es.emit);
            serde_json::json!({
                "value": value,
                "label": label,
                "agentName": es.agent_name,
                "bindingName": es.binding_name,
                "description": es.description,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({ "sources": sources })))
}

/// GET /agents/active — returns currently active agents from the AgentRegistry.
pub async fn list_active_agents(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let registry = state.agent_registry.read().await;
    let now = chrono::Utc::now();

    let agents: Vec<serde_json::Value> = registry.values().map(|agent| {
        // Fetch description from DB if available
        let description = state.store.get_agent(&agent.agent_id)
            .ok()
            .flatten()
            .map(|r| r.description)
            .unwrap_or_default();

        // Compute nextFireAt: earliest next fire across all active bindings
        let next_fire_at = compute_next_fire(&state.store, &agent.agent_id, &now);

        serde_json::json!({
            "agentId": agent.agent_id,
            "name": agent.name,
            "description": description,
            "channelId": agent.channel_id,
            "hasConfig": agent.config.is_some(),
            "workflowCount": agent.config.as_ref().map(|c| c.workflows.len()).unwrap_or(0),
            "skillCount": agent.config.as_ref().map(|c| c.skills.len()).unwrap_or(0),
            "nextFireAt": next_fire_at,
        })
    }).collect();

    Ok(Json(serde_json::json!({
        "agents": agents,
        "count": agents.len(),
    })))
}

/// Compute the earliest next fire timestamp across all active bindings for an agent.
fn compute_next_fire(store: &db::Store, agent_id: &str, now: &chrono::DateTime<chrono::Utc>) -> Option<i64> {
    let bindings = store.list_agent_workflows(agent_id).ok()?;
    let mut earliest: Option<i64> = None;
    let now_ts = now.timestamp();

    for binding in &bindings {
        if binding.is_active == 0 {
            continue;
        }

        let next_ts = match binding.trigger_type.as_str() {
            "schedule" => {
                // Parse cron and find the next fire time that's in the future
                let schedule: cron::Schedule = match binding.trigger_config.parse() {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                // Always compute from now to get the next future fire time
                schedule.after(now).next().map(|t| t.timestamp())
            }
            "heartbeat" => {
                // Parse interval, compute next fire from last_fired
                let parts: Vec<&str> = binding.trigger_config.split('|').collect();
                let interval_str = parts.first().copied().unwrap_or("30m");
                let interval_secs = parse_interval_secs(interval_str);
                let last_fired = parse_last_fired(binding.last_fired.as_deref());
                let mut next = last_fired.timestamp() + interval_secs;

                // If next is in the past, advance to the next future interval
                while next <= now_ts {
                    next += interval_secs;
                }

                Some(next)
            }
            _ => None, // event/manual — no scheduled fire
        };

        if let Some(ts) = next_ts {
            earliest = Some(earliest.map_or(ts, |e: i64| e.min(ts)));
        }
    }

    earliest
}

fn parse_last_fired(s: Option<&str>) -> chrono::DateTime<chrono::Utc> {
    s.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .or_else(|| {
            s.and_then(|s| s.parse::<i64>().ok())
                .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0))
        })
        .unwrap_or(chrono::DateTime::UNIX_EPOCH)
}

fn parse_interval_secs(s: &str) -> i64 {
    let s = s.trim();
    if let Some(rest) = s.strip_suffix('s') {
        rest.parse().unwrap_or(60)
    } else if let Some(rest) = s.strip_suffix('m') {
        rest.parse::<i64>().unwrap_or(30) * 60
    } else if let Some(rest) = s.strip_suffix('h') {
        rest.parse::<i64>().unwrap_or(1) * 3600
    } else {
        // Bare number = minutes
        s.parse::<i64>().unwrap_or(30) * 60
    }
}

/// GET /agents/{id}/workflows — returns workflow bindings for an agent.
///
/// Returns a map keyed by binding_name with structured trigger objects
/// and full activity/connection data for the V2 workflow builder.
pub async fn list_agent_workflows(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    // Verify agent exists
    state
        .store
        .get_agent(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let workflows = state.store.list_agent_workflows(&id).map_err(to_error_response)?;

    // Build a map keyed by binding_name with structured trigger
    let mut wf_map = serde_json::Map::new();
    for wf in &workflows {
        let trigger = reconstruct_trigger(&wf.trigger_type, &wf.trigger_config);
        let mut entry = serde_json::json!({
            "trigger": trigger,
            "description": wf.description,
            "isActive": wf.is_active != 0,
            "lastFired": wf.last_fired,
            "emit": wf.emit,
            "activities": wf.activities,
            "connections": wf.connections,
        });
        if let Some(inputs_str) = &wf.inputs {
            if let Ok(inputs) = serde_json::from_str::<serde_json::Value>(inputs_str) {
                entry["inputs"] = inputs;
            }
        }
        wf_map.insert(wf.binding_name.clone(), entry);
    }

    Ok(Json(serde_json::json!({
        "workflows": wf_map,
        "count": workflows.len(),
    })))
}

/// Reconstruct a structured trigger JSON from the flat trigger_type + trigger_config
/// stored in the DB, matching the V2 frontend's expected trigger shape.
fn reconstruct_trigger(trigger_type: &str, trigger_config: &str) -> serde_json::Value {
    match trigger_type {
        "schedule" => serde_json::json!({
            "type": "schedule",
            "cron": trigger_config,
            "schedule": cron_to_human_readable(trigger_config),
        }),
        "heartbeat" => {
            let parts: Vec<&str> = trigger_config.splitn(2, '|').collect();
            let interval = parts.first().unwrap_or(&"");
            let window = parts.get(1);
            let mut t = serde_json::json!({
                "type": "heartbeat",
                "interval": interval,
            });
            if let Some(w) = window {
                if let Some((start, end)) = w.split_once('-') {
                    t["window"] = serde_json::json!({ "start": start, "end": end });
                }
            }
            t
        }
        "event" => {
            let sources: Vec<&str> = trigger_config.split(',').filter(|s| !s.is_empty()).collect();
            serde_json::json!({
                "type": "event",
                "sources": sources,
            })
        }
        "watch" => {
            // trigger_config is JSON for watch triggers
            if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(trigger_config) {
                let mut t = serde_json::json!({ "type": "watch" });
                if let Some(obj) = cfg.as_object() {
                    for (k, v) in obj {
                        t[k] = v.clone();
                    }
                }
                t
            } else {
                serde_json::json!({ "type": "watch" })
            }
        }
        "manual" => serde_json::json!({ "type": "manual" }),
        _ => serde_json::json!({ "type": trigger_type }),
    }
}

/// Convert a cron expression to a human-readable schedule string.
fn cron_to_human_readable(cron: &str) -> String {
    let parts: Vec<&str> = cron.split_whitespace().collect();
    if parts.len() < 5 {
        return cron.to_string();
    }
    let (minute, hour, _dom, _month, dow) = (parts[0], parts[1], parts[2], parts[3], parts[4]);

    // Parse time
    let time_str = if let (Ok(h), Ok(m)) = (hour.parse::<u32>(), minute.parse::<u32>()) {
        let (h12, ampm) = if h == 0 {
            (12, "AM")
        } else if h < 12 {
            (h, "AM")
        } else if h == 12 {
            (12, "PM")
        } else {
            (h - 12, "PM")
        };
        if m == 0 {
            format!("{}:00 {}", h12, ampm)
        } else {
            format!("{}:{:02} {}", h12, m, ampm)
        }
    } else {
        return cron.to_string();
    };

    // Parse day-of-week
    let day_str = match dow {
        "*" => "daily",
        "1-5" | "MON-FRI" => "weekdays",
        "0,6" | "SAT,SUN" => "weekends",
        "1" | "MON" => "Monday",
        "2" | "TUE" => "Tuesday",
        "3" | "WED" => "Wednesday",
        "4" | "THU" => "Thursday",
        "5" | "FRI" => "Friday",
        "6" | "SAT" => "Saturday",
        "0" | "7" | "SUN" => "Sunday",
        _ => return format!("{} ({})", time_str, cron),
    };

    format!("{} {}", time_str, day_str)
}

/// POST /agents/{id}/check-update — check if a newer version is available on NeboLoop.
pub async fn check_agent_update(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let agent = state.store.get_agent(&id).map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let kind = agent.kind.as_deref().unwrap_or("");
    if kind.is_empty() {
        return Ok(Json(serde_json::json!({
            "hasUpdate": false,
            "message": "User-created agent — no marketplace updates available",
        })));
    }

    let api = crate::codes::build_api_client(&state).map_err(to_error_response)?;
    match api.get_skill(&id).await {
        Ok(detail) => {
            let remote_version = &detail.item.version;
            let local_version = agent.napp_path.as_ref()
                .and_then(|p| {
                    let manifest_path = std::path::PathBuf::from(p).join("manifest.json");
                    std::fs::read_to_string(manifest_path).ok()
                })
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| v["version"].as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "unknown".to_string());

            let has_update = !remote_version.is_empty() && remote_version != &local_version;
            Ok(Json(serde_json::json!({
                "hasUpdate": has_update,
                "localVersion": local_version,
                "remoteVersion": remote_version,
            })))
        }
        Err(e) => Ok(Json(serde_json::json!({
            "hasUpdate": false,
            "error": e.to_string(),
        }))),
    }
}

/// POST /agents/{id}/apply-update — download and apply the latest version from NeboLoop.
pub async fn apply_agent_update(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let agent = state.store.get_agent(&id).map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let kind = agent.kind.as_deref().unwrap_or("");
    if kind.is_empty() {
        return Err(to_error_response(types::NeboError::Validation(
            "Cannot update a user-created agent from marketplace".to_string()
        )));
    }

    let api = crate::codes::build_api_client(&state).map_err(to_error_response)?;
    match tools::persist_agent_from_api(&api, &id, &agent.name, kind, &state.store).await {
        Ok(_) => {
            // Re-read updated agent
            let updated = state.store.get_agent(&id).map_err(to_error_response)?
                .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

            // Update live registry
            if let Some(config) = if !updated.frontmatter.is_empty() {
                napp::agent::parse_agent_config(&updated.frontmatter).ok()
            } else { None } {
                let active = tools::ActiveAgent {
                    agent_id: id.clone(),
                    name: updated.name.clone(),
                    agent_md: updated.agent_md.clone(),
                    config: Some(config),
                    channel_id: None,
                    degraded: None,
                };
                state.agent_registry.write().await.insert(id.clone(), active);
            }

            Ok(Json(serde_json::json!({
                "ok": true,
                "agent": updated,
            })))
        }
        Err(e) => Err(to_error_response(types::NeboError::Internal(format!("Update failed: {}", e)))),
    }
}

/// POST /agents/{id}/reload — re-read AGENT.md + agent.json from filesystem and sync to DB.
pub async fn reload_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let agent = state.store.get_agent(&id).map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let agent_dir = agent.napp_path.as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            let data = config::data_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            data.join("user").join("agents").join(&agent.name)
        });

    if !agent_dir.exists() {
        return Err(to_error_response(types::NeboError::NotFound));
    }

    let mut changes = Vec::new();
    let mut current_md = agent.agent_md.clone();
    let mut current_fm = agent.frontmatter.clone();

    // Reload AGENT.md
    let md_path = agent_dir.join("AGENT.md");
    if md_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&md_path) {
            if content != current_md {
                current_md = content;
                changes.push("AGENT.md");
            }
        }
    }

    // Reload agent.json
    let json_path = agent_dir.join("agent.json");
    if json_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&json_path) {
            if content.trim() != current_fm.trim() {
                if napp::agent::parse_agent_config(&content).is_ok() {
                    current_fm = content;
                    changes.push("agent.json");
                }
            }
        }
    }

    if changes.is_empty() {
        return Ok(Json(serde_json::json!({ "ok": true, "message": "Already in sync" })));
    }

    // Persist
    state.store.update_agent(&id, &agent.name, &agent.description, &current_md, &current_fm, agent.pricing_model.as_deref(), agent.pricing_cost)
        .map_err(to_error_response)?;

    // Re-register triggers if agent.json changed
    if changes.contains(&"agent.json") {
        if let Ok(config) = napp::agent::parse_agent_config(&current_fm) {
            let _ = state.store.delete_cron_jobs_by_prefix(&format!("agent-{}-", id));
            let _ = state.store.delete_agent_workflows(&id);
            process_agent_bindings(&id, &config, &state).await;
        }
    }

    // Update live registry
    if let Some(active) = state.agent_registry.write().await.get_mut(&id) {
        active.agent_md = current_md;
        active.config = napp::agent::parse_agent_config(&current_fm).ok();
    }

    let updated = state.store.get_agent(&id).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "reloaded": changes,
        "agent": updated,
    })))
}

/// POST /agents/{id}/setup — broadcast a setup event to open the wizard on the frontend.
pub async fn trigger_agent_setup(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let agent = state.store.get_agent(&id).map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    state.hub.broadcast("agent_setup", serde_json::json!({
        "agentId": agent.id,
        "agentName": agent.name,
        "agentDescription": agent.description,
    }));

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// PUT /agents/{id}/inputs — update user-supplied input values for an agent.
pub async fn update_agent_inputs(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .get_agent(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let values_str = body.to_string();
    state.store.update_agent_input_values(&id, &values_str).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /agents/{id}/stats — aggregate workflow run stats for an agent.
pub async fn agent_stats(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .get_agent(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let stats = state.store.agent_workflow_stats(&id).map_err(to_error_response)?;
    let errors = state.store.agent_recent_errors(&id, 5).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({
        "stats": stats,
        "recentErrors": errors,
    })))
}

/// GET /agents/{id}/runs — list workflow runs for an agent.
pub async fn list_agent_runs(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ListQuery>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .get_agent(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let wf_id = format!("agent:{}", id);
    let runs = state.store.list_workflow_runs(&wf_id, q.limit, q.offset).map_err(to_error_response)?;
    let total = state.store.count_workflow_runs(&wf_id).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({
        "runs": runs,
        "total": total,
    })))
}

/// POST /agents/{id}/activate — activate an agent from the REST API (makes it appear in sidebar).
pub async fn activate_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let agent = state
        .store
        .get_agent(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    // Persist enabled state so it survives restart
    state.store.set_agent_enabled(&id, true).map_err(to_error_response)?;

    let agent_id = agent.id.clone();
    let config = if !agent.frontmatter.is_empty() {
        napp::agent::parse_agent_config(&agent.frontmatter).ok()
    } else {
        None
    };

    let active = tools::ActiveAgent {
        agent_id: agent_id.clone(),
        name: agent.name.clone(),
        agent_md: agent.agent_md.clone(),
        config,
        channel_id: None,
        degraded: None,
    };

    state.agent_registry.write().await.insert(agent_id.clone(), active);

    // Start autonomous agent worker (heartbeat, event, schedule triggers)
    state.agent_workers.start_agent(&agent_id, &agent.name).await;

    // Register agent in the owner's personal loop (non-blocking)
    {
        let st = state.clone();
        let name = agent.name.clone();
        let slug = agent.name.to_lowercase().replace(' ', "-");
        tokio::spawn(async move {
            if let Err(e) = crate::codes::register_agent_in_loop(&st, &name, &slug).await {
                warn!(agent = %name, error = %e, "failed to register agent in loop");
            }
        });
    }

    state.hub.broadcast(
        "agent_activated",
        serde_json::json!({ "agentId": agent_id, "name": agent.name }),
    );

    Ok(Json(serde_json::json!({
        "agentId": agent_id,
        "name": agent.name,
        "status": "active",
    })))
}

/// POST /agents/{id}/deactivate — deactivate an agent from the REST API.
pub async fn deactivate_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    // Persist disabled state so it survives restart
    if let Err(e) = state.store.set_agent_enabled(&id, false) {
        warn!(agent = %id, error = %e, "failed to persist agent disabled state");
    }

    // Stop autonomous agent worker (cancels heartbeat, event, schedule triggers)
    state.agent_workers.stop_agent(&id).await;

    let removed = state.agent_registry.write().await.remove(&id);
    match removed {
        Some(agent) => {
            // Deregister agent from the owner's personal loop (non-blocking)
            {
                let st = state.clone();
                let slug = agent.name.to_lowercase().replace(' ', "-");
                tokio::spawn(async move {
                    if let Err(e) = crate::codes::deregister_agent_from_loop(&st, &slug).await {
                        warn!(agent = %slug, error = %e, "failed to deregister agent from loop");
                    }
                });
            }

            state.hub.broadcast(
                "agent_deactivated",
                serde_json::json!({ "agentId": id, "name": agent.name }),
            );
            Ok(Json(serde_json::json!({
                "agentId": id,
                "name": agent.name,
                "status": "deactivated",
            })))
        }
        None => Err(to_error_response(types::NeboError::NotFound)),
    }
}

/// POST /agents/{id}/duplicate — create a copy of an agent and auto-activate it.
pub async fn duplicate_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let source = state
        .store
        .get_agent(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let new_id = uuid::Uuid::new_v4().to_string();
    let new_name = format!("{} (Copy)", source.name);

    // Update frontmatter name in agent_md
    let new_agent_md = if source.agent_md.contains("name:") {
        // Replace just the first name: line in the YAML frontmatter
        let mut result = String::new();
        let mut replaced = false;
        for line in source.agent_md.lines() {
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
        source.agent_md.clone()
    };

    let agent = state
        .store
        .create_agent(
            &new_id,
            source.kind.as_deref(),
            &new_name,
            &source.description,
            &new_agent_md,
            &source.frontmatter,
            source.pricing_model.as_deref(),
            source.pricing_cost,
        )
        .map_err(to_error_response)?;

    // Copy agent_workflow bindings from source
    let source_workflows = state.store.list_agent_workflows(&id).map_err(to_error_response)?;
    for wf in &source_workflows {
        let activities_str = wf.activities.as_ref().map(|v| v.to_string());
        let connections_str = wf.connections.as_ref().map(|v| v.to_string());
        let _ = state.store.upsert_agent_workflow(
            &new_id,
            &wf.binding_name,
            &wf.trigger_type,
            &wf.trigger_config,
            wf.description.as_deref(),
            wf.inputs.as_deref(),
            wf.emit.as_deref(),
            activities_str.as_deref(),
            connections_str.as_deref(),
        );
    }

    // Auto-activate
    let active = tools::ActiveAgent {
        agent_id: new_id.clone(),
        name: agent.name.clone(),
        agent_md: agent.agent_md.clone(),
        config: None,
        channel_id: None,
        degraded: None,
    };
    state.agent_registry.write().await.insert(new_id.clone(), active);
    state.agent_workers.start_agent(&new_id, &agent.name).await;

    state.hub.broadcast(
        "agent_installed",
        serde_json::json!({ "agentId": &new_id, "name": &agent.name }),
    );
    state.hub.broadcast(
        "agent_activated",
        serde_json::json!({ "agentId": &new_id, "name": &agent.name }),
    );

    Ok(Json(serde_json::json!({
        "agent": { "id": new_id, "name": agent.name },
        "activated": true,
    })))
}

/// POST /agents/{id}/chat — send a message to an agent via the unified chat pipeline.
pub async fn chat_with_agent(
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

    // Verify agent is active
    {
        let reg = state.agent_registry.read().await;
        if !reg.contains_key(&id) {
            return Err(to_error_response(types::NeboError::Validation(
                format!("Agent '{}' is not active. Activate it first.", id),
            )));
        }
    }

    let session_key = agent::keyparser::build_agent_session_key(&id, "web");

    let entity_config = crate::entity_config::resolve_for_chat(&state.store, "agent", &id);

    let config = crate::chat_dispatch::ChatConfig {
        session_key: session_key.clone(),
        prompt,
        system: String::new(),
        user_id: String::new(),
        channel: "web".to_string(),
        origin: tools::Origin::User,
        agent_id: id.clone(),
        cancel_token: tokio_util::sync::CancellationToken::new(),
        lane: types::constants::lanes::MAIN.to_string(),
        comm_reply: None,
        entity_config,
        images: vec![],
        entity_name: String::new(),
        origin_agent_id: None,
        mention_context: None,
    };

    crate::chat_dispatch::run_chat(&state, config).await;

    Ok(Json(serde_json::json!({
        "sessionId": session_key,
        "agentId": id,
        "status": "dispatched",
    })))
}

// ── Workflow Binding CRUD ─────────────────────────────────────────────────────

/// Build trigger JSON for agent.json from flat (type, config) pair.
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
        "schedule" => {
            let raw = trigger_config.get("cron").and_then(|v| v.as_str()).unwrap_or("");
            tools::PersonaTool::normalize_cron(raw)
        }
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

/// Write updated frontmatter back to filesystem agent.json if napp_path exists.
fn write_agent_json_to_fs(napp_path: &Option<String>, frontmatter: &serde_json::Value) {
    if let Some(path) = napp_path {
        let agent_json_path = std::path::Path::new(path).join("agent.json");
        if let Ok(pretty) = serde_json::to_string_pretty(frontmatter) {
            if let Err(e) = std::fs::write(&agent_json_path, &pretty) {
                warn!(path = %agent_json_path.display(), error = %e, "failed to write agent.json");
            }
        }
    }
}

/// Register triggers (schedule cron + event subscriptions) for a single binding.
async fn register_binding_triggers(
    agent_id: &str,
    binding_name: &str,
    trigger_type: &str,
    trigger_config_flat: &str,
    frontmatter: &serde_json::Value,
    state: &AppState,
) {
    if trigger_type == "schedule" {
        let name = format!("agent-{}-{}", agent_id, binding_name);
        let command = format!("agent:{}:{}", agent_id, binding_name);
        if let Err(e) = state.store.upsert_cron_job(
            &name, trigger_config_flat, &command, "agent_workflow", None, None, None, true,
        ) {
            warn!(agent = agent_id, binding = binding_name, error = %e, "failed to register cron job");
        }
    } else if trigger_type == "event" {
        // Build event subscriptions from the binding definition in frontmatter
        let binding_val = frontmatter
            .get("workflows")
            .and_then(|w| w.get(binding_name));

        let parsed_binding = binding_val
            .and_then(|v| serde_json::from_value::<napp::agent::WorkflowBinding>(v.clone()).ok());

        let def_json = parsed_binding.as_ref()
            .filter(|wb| wb.has_activities())
            .map(|wb| wb.to_workflow_json(binding_name));

        // Build emit_source from binding emit field
        let emit_source = parsed_binding.as_ref()
            .and_then(|wb| wb.emit.as_ref())
            .map(|emit_name| {
                let agent_name = frontmatter.get("name").and_then(|n| n.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| state.store.get_agent(agent_id).ok().flatten().map(|r| r.name))
                    .unwrap_or_else(|| agent_id.to_string());
                let slug = agent_name.to_lowercase().replace(' ', "-");
                format!("{}.{}", slug, emit_name)
            });

        for source in trigger_config_flat.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            state.event_dispatcher.subscribe(
                workflow::events::EventSubscription {
                    pattern: source.to_string(),
                    default_inputs: serde_json::Value::Object(Default::default()),
                    agent_source: agent_id.to_string(),
                    binding_name: binding_name.to_string(),
                    definition_json: def_json.clone(),
                    emit_source: emit_source.clone(),
                },
            ).await;
        }
    }
}

/// POST /agents/{id}/workflows — create a new workflow binding.
pub async fn create_agent_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let agent = state.store.get_agent(&id).map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let binding_name = body["bindingName"].as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("bindingName required".into())))?;
    let trigger_type = body["triggerType"].as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("triggerType required".into())))?;
    let trigger_config = body.get("triggerConfig").cloned().unwrap_or(serde_json::json!({}));

    // Parse existing frontmatter
    let mut fm: serde_json::Value = serde_json::from_str(&agent.frontmatter).unwrap_or(serde_json::json!({}));

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

    // Update agent in DB
    state.store.update_agent(
        &id, &agent.name, &agent.description, &agent.agent_md,
        &fm.to_string(), agent.pricing_model.as_deref(), agent.pricing_cost,
    ).map_err(to_error_response)?;

    // Upsert tracking row
    let trigger_config_flat = flatten_trigger_config(trigger_type, &trigger_config);
    let desc = body.get("description").and_then(|v| v.as_str());
    let inputs_json = body.get("inputs").and_then(|v| serde_json::to_string(v).ok());
    let emit_val = body.get("emit").and_then(|v| v.as_str());
    let activities_json = body.get("activities").and_then(|v| serde_json::to_string(v).ok());
    let connections_json = body.get("connections").and_then(|v| serde_json::to_string(v).ok());
    state.store.upsert_agent_workflow(
        &id, binding_name, trigger_type, &trigger_config_flat,
        desc, inputs_json.as_deref(), emit_val, activities_json.as_deref(),
        connections_json.as_deref(),
    ).map_err(to_error_response)?;

    // Register triggers
    register_binding_triggers(&id, binding_name, trigger_type, &trigger_config_flat, &fm, &state).await;

    // Write to filesystem
    write_agent_json_to_fs(&agent.napp_path, &fm);

    let workflows = state.store.list_agent_workflows(&id).map_err(to_error_response)?;
    let wf = workflows.iter().find(|w| w.binding_name == binding_name);

    Ok(Json(serde_json::json!({
        "workflow": wf,
    })))
}

/// PUT /agents/{id}/workflows/{binding_name} — update an existing workflow binding.
pub async fn update_agent_workflow(
    State(state): State<AppState>,
    Path((id, binding_name)): Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let agent = state.store.get_agent(&id).map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let mut fm: serde_json::Value = serde_json::from_str(&agent.frontmatter).unwrap_or(serde_json::json!({}));

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

    // Update agent in DB
    state.store.update_agent(
        &id, &agent.name, &agent.description, &agent.agent_md,
        &fm.to_string(), agent.pricing_model.as_deref(), agent.pricing_cost,
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
    let connections_json = fm["workflows"][&binding_name].get("connections").and_then(|v| serde_json::to_string(v).ok());
    state.store.upsert_agent_workflow(
        &id, &binding_name, new_trigger_type, &trigger_config_flat,
        desc, inputs_json.as_deref(), emit_val, activities_json.as_deref(),
        connections_json.as_deref(),
    ).map_err(to_error_response)?;

    // If trigger type changed, unregister old triggers first
    if body.get("triggerType").is_some() {
        workflow::triggers::unregister_single_agent_trigger(&id, &binding_name, &state.store);
        state.event_dispatcher.unsubscribe_binding(&id, &binding_name).await;
    }

    // Register new triggers
    register_binding_triggers(&id, &binding_name, new_trigger_type, &trigger_config_flat, &fm, &state).await;

    // Write to filesystem
    write_agent_json_to_fs(&agent.napp_path, &fm);

    let workflows = state.store.list_agent_workflows(&id).map_err(to_error_response)?;
    let wf = workflows.iter().find(|w| w.binding_name == binding_name);

    Ok(Json(serde_json::json!({
        "workflow": wf,
    })))
}

/// POST /agents/{id}/workflows/{binding_name}/toggle — toggle a workflow binding on/off.
pub async fn toggle_agent_workflow(
    State(state): State<AppState>,
    Path((id, binding_name)): Path<(String, String)>,
) -> HandlerResult<serde_json::Value> {
    // Verify agent exists
    let agent = state.store.get_agent(&id).map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let is_active = state.store.toggle_agent_workflow(&id, &binding_name).map_err(to_error_response)?;

    if is_active {
        // Re-register triggers
        let fm: serde_json::Value = serde_json::from_str(&agent.frontmatter).unwrap_or_default();
        if let Ok(bindings) = state.store.list_agent_workflows(&id) {
            if let Some(binding) = bindings.iter().find(|b| b.binding_name == binding_name) {
                register_binding_triggers(
                    &id, &binding_name, &binding.trigger_type, &binding.trigger_config, &fm, &state,
                ).await;
            }
        }
    } else {
        // Unregister triggers
        workflow::triggers::unregister_single_agent_trigger(&id, &binding_name, &state.store);
        state.event_dispatcher.unsubscribe_binding(&id, &binding_name).await;
    }

    Ok(Json(serde_json::json!({
        "bindingName": binding_name,
        "isActive": is_active,
    })))
}

/// DELETE /agents/{id}/workflows/{binding_name} — delete a workflow binding.
pub async fn delete_agent_workflow(
    State(state): State<AppState>,
    Path((id, binding_name)): Path<(String, String)>,
) -> HandlerResult<serde_json::Value> {
    let agent = state.store.get_agent(&id).map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    // Remove from frontmatter
    let mut fm: serde_json::Value = serde_json::from_str(&agent.frontmatter).unwrap_or(serde_json::json!({}));
    if let Some(workflows) = fm.get_mut("workflows").and_then(|w| w.as_object_mut()) {
        workflows.remove(&binding_name);
    }

    // Update agent in DB
    state.store.update_agent(
        &id, &agent.name, &agent.description, &agent.agent_md,
        &fm.to_string(), agent.pricing_model.as_deref(), agent.pricing_cost,
    ).map_err(to_error_response)?;

    // Delete tracking row
    state.store.delete_single_agent_workflow(&id, &binding_name).map_err(to_error_response)?;

    // Unregister triggers
    workflow::triggers::unregister_single_agent_trigger(&id, &binding_name, &state.store);
    state.event_dispatcher.unsubscribe_binding(&id, &binding_name).await;

    // Write to filesystem
    write_agent_json_to_fs(&agent.napp_path, &fm);

    Ok(Json(serde_json::json!({
        "message": format!("Binding '{}' deleted", binding_name),
    })))
}

/// GET /agents/{id}/surfaces — Return A2UI replay messages for active surfaces.
///
/// If no active surfaces exist but the agent has a `default` view in views.json,
/// auto-creates the surface server-side so it persists across page refreshes.
pub async fn get_agent_surfaces(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let existing = state.a2ui.list_agent_surfaces(&id).await;

    if existing.is_empty() {
        // Check if agent has a views.json with a "default" view
        if let Some(views) = get_agent_views(&state, &id).await {
            if let Some(default_view) = views.get("default") {
                if let Some(components) = default_view.get("components").and_then(|c| c.as_array()) {
                    let catalog_id = "https://a2ui.org/specification/v0_9/basic_catalog.json";
                    let surface_type = default_view
                        .get("surface_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("panel");

                    match state.a2ui.create_surface(&id, "default", surface_type, catalog_id, None).await {
                        Ok(sid) => {
                            if let Err(e) = state.a2ui.update_components(&sid, components.clone()).await {
                                warn!(error = %e, "failed to push default view components");
                            }
                            // If view has initial data, push it too
                            if let Some(data) = default_view.get("data") {
                                if let Err(e) = state.a2ui.update_data_model(&sid, None, data.clone()).await {
                                    warn!(error = %e, "failed to push default view data model");
                                }
                            }
                            // Start data bindings if defined
                            state.a2ui.start_data_bindings(&sid, default_view).await;
                        }
                        Err(e) => {
                            warn!(error = %e, agent_id = %id, "failed to auto-create default surface");
                        }
                    }
                }
            }
        }
    }

    let messages = state.a2ui.get_agent_replay_messages(&id).await;
    Ok(Json(serde_json::json!({
        "messages": messages,
    })))
}

/// GET /agents/{id}/theme.css — Return the agent's theme CSS.
///
/// Returns the theme.css content from the agent's LoadedAgent (filesystem).
/// 204 No Content if the agent has no theme.css.
pub async fn get_agent_theme(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl axum::response::IntoResponse {
    // Try DB agent → loader by name
    if let Ok(Some(agent)) = state.store.get_agent(&id) {
        if let Some(loaded) = state.agent_loader.get_by_name(&agent.name).await {
            if let Some(ref css) = loaded.theme_css {
                return axum::response::Response::builder()
                    .status(200)
                    .header("content-type", "text/css; charset=utf-8")
                    .header("cache-control", "no-cache")
                    .body(axum::body::Body::from(css.clone()))
                    .unwrap();
            }
        }
    }

    // Last resort: try id as agent name
    if let Some(loaded) = state.agent_loader.get_by_name(&id).await {
        if let Some(ref css) = loaded.theme_css {
            return axum::response::Response::builder()
                .status(200)
                .header("content-type", "text/css; charset=utf-8")
                .header("cache-control", "no-cache")
                .body(axum::body::Body::from(css.clone()))
                .unwrap();
        }
    }

    axum::response::Response::builder()
        .status(204)
        .body(axum::body::Body::empty())
        .unwrap()
}

/// Return the agent's `_nav` config from views.json.
///
/// If `_nav` is explicitly defined, returns it. Otherwise auto-generates
/// nav items from the top-level view keys (excluding underscore-prefixed keys
/// and child views like "document-review").
pub async fn get_agent_nav(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let views = get_agent_views(&state, &id).await;
    let Some(views) = views else {
        return Ok(axum::Json(serde_json::json!([])));
    };

    // Check for explicit _nav config
    if let Some(nav) = views.get("_nav") {
        return Ok(axum::Json(nav.clone()));
    }

    // Auto-generate from view keys
    let obj = views.as_object().cloned().unwrap_or_default();
    let items: Vec<serde_json::Value> = obj
        .keys()
        .filter(|k| !k.starts_with('_'))
        .map(|k| {
            let label = k
                .replace('-', " ")
                .split_whitespace()
                .map(|w| {
                    let mut c = w.chars();
                    match c.next() {
                        None => String::new(),
                        Some(f) => f.to_uppercase().to_string() + c.as_str(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            serde_json::json!({
                "viewId": k,
                "label": if k == "default" { "Dashboard" } else { &label },
            })
        })
        .collect();

    Ok(axum::Json(serde_json::json!(items)))
}

/// Look up an agent's views.json content.
///
/// Tries two sources:
/// 1. DB agent record's napp_path (filesystem path to agent directory)
/// 2. Agent loader's in-memory LoadedAgent (already parsed views.json)
pub async fn get_agent_views(state: &AppState, agent_id: &str) -> Option<serde_json::Value> {
    // Try DB agent's napp_path first
    if let Ok(Some(agent)) = state.store.get_agent(agent_id) {
        if let Some(napp_path) = agent.napp_path.as_ref() {
            if !napp_path.is_empty() {
                let views_path = std::path::PathBuf::from(napp_path).join("views.json");
                if let Ok(content) = std::fs::read_to_string(&views_path) {
                    if let Ok(views) = serde_json::from_str(&content) {
                        return Some(views);
                    }
                }
            }
        }

        // Fallback: look up via agent loader by name
        if let Some(loaded) = state.agent_loader.get_by_name(&agent.name).await {
            return loaded.views;
        }
    }

    // Last resort: try agent_id as a name directly
    if let Some(loaded) = state.agent_loader.get_by_name(agent_id).await {
        return loaded.views;
    }

    None
}

// ── Agent Multi-Chat ─────────────────────────────────────────────────────────

/// GET /api/v1/agents/{id}/chats — list all chats for an agent.
pub async fn list_agent_chats(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let session_key = agent::keyparser::build_agent_session_key(&id, "web");
    let mut chats = state.store.list_chats_by_session(&session_key).unwrap_or_default();

    // Resolve active chat_id from session (if session exists).
    let active_chat_id = state.runner.sessions()
        .resolve_session_id_by_key(&session_key)
        .ok()
        .map(|sid| state.runner.sessions().active_chat_id(&sid))
        .unwrap_or_default();

    // Backfill: legacy agent chats store messages under the session key as chat_id
    // but have no `chats` row. If we found no chats but messages exist, create the row.
    if chats.is_empty() {
        let legacy_chat_id = if active_chat_id.is_empty() { &session_key } else { &active_chat_id };
        let msg_count = state.store.count_chat_messages(legacy_chat_id).unwrap_or(0);
        if msg_count > 0 {
            if let Ok(chat) = state.store.create_chat_for_session(
                legacy_chat_id,
                &session_key,
                "Chat 1",
                None,
            ) {
                chats.push(chat);
            }
        }
    }

    // Enrich chats with name, preview, updatedAt (relative), and messages count
    let now = chrono::Utc::now().timestamp();
    let enriched: Vec<serde_json::Value> = chats.iter().map(|chat| {
        let msg_count = state.store.count_chat_messages(&chat.id).unwrap_or(0);
        // Get last message for preview
        let preview = state.store
            .get_chat_messages_budgeted(&chat.id, 200, None)
            .ok()
            .and_then(|msgs| msgs.last().map(|m| {
                let content = m.content.chars().take(120).collect::<String>();
                if m.content.len() > 120 { format!("{}...", content) } else { content }
            }))
            .unwrap_or_default();
        let updated_at_relative = format_relative_time(chat.updated_at, now);
        serde_json::json!({
            "id": chat.id,
            "name": chat.title,
            "title": chat.title,
            "preview": preview,
            "updatedAt": updated_at_relative,
            "messages": msg_count,
            "createdAt": chat.created_at,
            "updatedAtEpoch": chat.updated_at,
            "sessionName": chat.session_name,
        })
    }).collect();

    let total = enriched.len();
    Ok(Json(serde_json::json!({
        "chats": enriched,
        "activeChatId": active_chat_id,
        "total": total,
    })))
}

/// Format an epoch timestamp as a relative time string.
fn format_relative_time(epoch: i64, now: i64) -> String {
    let diff = now - epoch;
    if diff < 60 {
        "just now".to_string()
    } else if diff < 3600 {
        let mins = diff / 60;
        format!("{}m ago", mins)
    } else if diff < 86400 {
        let hours = diff / 3600;
        format!("{}h ago", hours)
    } else if diff < 604800 {
        let days = diff / 86400;
        format!("{}d ago", days)
    } else {
        chrono::DateTime::from_timestamp(epoch, 0)
            .map(|dt| dt.format("%b %d").to_string())
            .unwrap_or_default()
    }
}

/// POST /api/v1/agents/{id}/chats — create a new chat under the agent's session.
pub async fn create_new_agent_chat(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let session_key = agent::keyparser::build_agent_session_key(&id, "web");

    // Ensure the session exists (get_or_create lazily creates it).
    let session = state.runner.sessions()
        .get_or_create(&session_key, "")
        .map_err(to_error_response)?;

    let new_chat_id = state.runner.sessions()
        .rotate_chat(&session.id, None)
        .map_err(to_error_response)?;

    let chat = state.store.get_chat(&new_chat_id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    Ok(Json(serde_json::json!({
        "chat": chat,
        "messages": [],
        "totalMessages": 0,
        "sessionKey": session_key,
    })))
}

/// POST /api/v1/agents/{id}/chats/{chat_id}/activate — switch to an existing chat.
pub async fn activate_agent_chat(
    State(state): State<AppState>,
    Path((id, chat_id)): Path<(String, String)>,
) -> HandlerResult<serde_json::Value> {
    let session_key = agent::keyparser::build_agent_session_key(&id, "web");

    let session_id = state.runner.sessions()
        .resolve_session_id_by_key(&session_key)
        .map_err(to_error_response)?;

    state.runner.sessions()
        .set_active_chat(&session_id, &chat_id)
        .map_err(to_error_response)?;

    let mut messages = state.store
        .get_chat_messages_budgeted(&chat_id, 12000, None)
        .unwrap_or_default();
    super::chat::build_message_metadata(&mut messages);
    let total = state.store.count_chat_messages(&chat_id).unwrap_or(messages.len() as i64);

    Ok(Json(serde_json::json!({
        "chatId": chat_id,
        "messages": messages,
        "totalMessages": total,
        "sessionKey": session_key,
    })))
}
