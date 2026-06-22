use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::Deserialize;
use tracing::{info, warn};

use super::{HandlerResult, to_error_response};
use crate::state::AppState;
use tools::workflows::WorkflowManager;

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

pub(crate) fn app_tool_dir(agent: &db::models::Agent) -> Option<std::path::PathBuf> {
    if let Some(ref ui_path) = agent.app_ui_path {
        return std::path::Path::new(ui_path)
            .parent()
            .map(|p| p.to_path_buf());
    }
    let binary = agent.app_binary_path.as_ref()?;
    let path = std::path::PathBuf::from(binary);
    match path.parent() {
        Some(parent) if parent.file_name().and_then(|n| n.to_str()) == Some("bin") => {
            parent.parent().map(|p| p.to_path_buf())
        }
        Some(parent) => Some(parent.to_path_buf()),
        None => None,
    }
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
    mapping
        .into_iter()
        .filter_map(|(k, v)| {
            let key = match k {
                serde_yaml::Value::String(s) => s,
                _ => return None,
            };
            let value = match v {
                serde_yaml::Value::String(s) => serde_json::json!(s),
                serde_yaml::Value::Number(n) => serde_json::json!(n.to_string()),
                serde_yaml::Value::Bool(b) => serde_json::json!(b.to_string()),
                serde_yaml::Value::Sequence(seq) => {
                    let items: Vec<String> = seq
                        .into_iter()
                        .filter_map(|item| match item {
                            serde_yaml::Value::String(s) => Some(s),
                            other => Some(format!("{:?}", other)),
                        })
                        .collect();
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
        })
        .collect()
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
    Query(_q): Query<ListQuery>,
) -> HandlerResult<serde_json::Value> {
    // Filesystem is the source of truth — read from AgentLoader (in-memory, loaded from disk).
    // DB supplements with runtime state (is_enabled, input_values, installed_at).
    let fs_agents = state.agent_loader.list().await;

    // Build a lookup map from DB for supplemental state
    let db_map: std::collections::HashMap<String, db::models::Agent> = state
        .store
        .list_agents(1000, 0)
        .unwrap_or_default()
        .into_iter()
        .flat_map(|a| {
            let name_key = a.name.to_lowercase();
            let id_key = a.id.clone();
            vec![(id_key, a.clone()), (name_key, a)]
        })
        .collect();

    let mut agents = Vec::with_capacity(fs_agents.len());
    for loaded in &fs_agents {
        let fs_id = loaded
            .id
            .clone()
            .unwrap_or_else(|| loaded.agent_def.name.clone());
        let source = match loaded.source {
            napp::AgentSource::Installed => "installed",
            napp::AgentSource::User => "user",
        };

        // Look up DB record by ID or name for supplemental state
        let db_row = db_map
            .get(&fs_id)
            .or_else(|| db_map.get(&loaded.agent_def.name.to_lowercase()));

        // Prefer DB ID (UUID) over filesystem-derived ID (agent name)
        let agent_id = db_row
            .map(|r| r.id.clone())
            .unwrap_or(fs_id);

        // Identity (name, description, color, handle) is DB-owned for user edits.
        // Fall back to the filesystem/embedded loader only when the DB has no row
        // or the DB value is empty (e.g. freshly seeded).
        let name = db_row
            .map(|r| r.name.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&loaded.agent_def.name)
            .to_string();
        let description = db_row
            .map(|r| r.description.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&loaded.description)
            .to_string();
        let color = db_row.and_then(|r| r.color.clone());
        let handle = db_row.and_then(|r| r.handle.clone());

        // Compute a display name: window title > first # heading from body > name
        let display_name = loaded
            .app_window_config
            .as_ref()
            .and_then(|wc| wc.title.clone())
            .filter(|t| !t.is_empty())
            .or_else(|| {
                loaded.agent_def.body.lines().find_map(|line| {
                    line.trim()
                        .strip_prefix("# ")
                        .map(|h| h.trim().to_string())
                        .filter(|h| !h.is_empty())
                })
            })
            .unwrap_or_else(|| name.clone());

        let mut entry = serde_json::json!({
            "id": agent_id,
            "name": name,
            "displayName": display_name,
            "description": description,
            "color": color,
            "handle": handle,
            "source": source,
            "version": loaded.version,
            "isApp": loaded.is_app,
            "isEnabled": db_row.map(|r| r.is_enabled != 0).unwrap_or(true),
            "inputValues": db_row.map(|r| r.input_values.as_str()).unwrap_or("{}"),
            "installedAt": db_row.map(|r| r.installed_at),
            "loopExposed": db_row.map(|r| r.loop_exposed != 0).unwrap_or(false),
        });
        // Derive needsSetup from config inputs vs stored input_values
        let needs_setup = if let Some(ref cfg) = loaded.config {
            if cfg.inputs.is_empty() {
                false
            } else {
                let current_values: serde_json::Value = serde_json::from_str(
                    db_row.map(|r| r.input_values.as_str()).unwrap_or("{}"),
                )
                .unwrap_or_default();
                cfg.inputs.iter().any(|inp| {
                    if !inp.required {
                        return false;
                    }
                    let key = if inp.key.is_empty() {
                        inp.name.as_deref().unwrap_or("")
                    } else {
                        &inp.key
                    };
                    if key.is_empty() {
                        return false;
                    }
                    match current_values.get(key) {
                        None => true,
                        Some(v) => v.is_null() || v.as_str().map_or(false, |s| s.is_empty()),
                    }
                })
            }
        } else {
            false
        };
        entry["needsSetup"] = serde_json::json!(needs_setup);
        if let Some(ref wc) = loaded.app_window_config {
            entry["appWindowConfig"] = serde_json::to_value(wc).unwrap_or_default();
        }
        agents.push(entry);
    }

    let total = agents.len() as i64;
    Ok(Json(serde_json::json!({
        "agents": agents,
        "total": total,
    })))
}

/// POST /agents
/// Write a user-owned agent's files to `user/agents/<name>/` (AGENT.md, agent.json,
/// manifest.json) and record its napp_path. Shared by create_agent and
/// duplicate_agent so there is one filesystem-write path for user agents.
fn write_user_agent_files(
    store: &db::Store,
    id: &str,
    name: &str,
    description: &str,
    agent_md: &str,
    agent_json: &str,
) {
    let Ok(user_dir) = config::user_dir() else {
        return;
    };
    let agent_dir = user_dir.join("agents").join(name);
    if std::fs::create_dir_all(&agent_dir).is_err() {
        return;
    }
    let _ = std::fs::write(agent_dir.join("AGENT.md"), agent_md);
    let _ = std::fs::write(agent_dir.join("agent.json"), agent_json);
    let manifest_path = agent_dir.join("manifest.json");
    if !manifest_path.exists() {
        let manifest = serde_json::json!({
            "id": id,
            "name": name,
            "version": "1.0.0",
            "type": "agent",
            "description": description,
        });
        let _ = std::fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest).unwrap_or_default(),
        );
    }
    let _ = store.set_agent_napp_path(id, &agent_dir.to_string_lossy());
}

pub async fn create_agent(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    // Blank agent: create a minimal agent and auto-activate it
    if body.get("blank").and_then(|v| v.as_bool()).unwrap_or(false) {
        return create_blank_agent(state).await;
    }

    let agent_md = body["agentMd"].as_str().ok_or_else(|| {
        to_error_response(types::NeboError::Validation("agentMd required".into()))
    })?;

    let (fm, _body) = parse_agent_md(agent_md).map_err(to_error_response)?;

    let id = uuid::Uuid::new_v4().to_string();
    let kind = body["kind"].as_str().or_else(|| body["code"].as_str());
    let name = if fm.name.is_empty() {
        body["name"].as_str().ok_or_else(|| {
            to_error_response(types::NeboError::Validation(
                "name required in body or frontmatter".into(),
            ))
        })?
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
                    obj.insert(
                        "pricing".into(),
                        serde_json::json!({
                            "model": p.model,
                            "cost": p.cost,
                        }),
                    );
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

    // Write AGENT.md and agent.json to user/agents/{name}/ for filesystem-based
    // loading. Prefer the original agentJson (triggers, workflow bindings) if
    // provided, else the merged frontmatter.
    let agent_json_content =
        extract_agent_json_str(&body).unwrap_or_else(|| frontmatter_json.to_string());
    write_user_agent_files(&state.store, &id, name, description, agent_md, &agent_json_content);

    // Process agent.json workflow bindings if provided
    let mut install_report = Vec::new();
    if let Some(agent_json_str) = extract_agent_json_str(&body) {
        if let Ok(agent_config) = napp::agent::parse_agent_config(&agent_json_str) {
            install_report = process_agent_bindings(&id, &agent_config, &state).await;
        }
    }

    // Refresh agent loader cache so the new agent appears in GET /agents immediately.
    state.agent_loader.load_all().await;

    state.hub.broadcast(
        "agent_installed",
        serde_json::json!({ "agentId": agent.id, "name": agent.name }),
    );

    // Cascade: resolve skill dependencies. Only marketplace-referenced skills are
    // separate installs — bare names are plugin-provided tool bindings (see
    // deps::extract_agent_deps).
    let mut deps = Vec::new();
    for s in &fm.skills {
        if crate::deps::is_marketplace_ref(s) {
            deps.push(crate::deps::DepRef::new(crate::deps::DepType::Skill, s.clone()));
        }
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
    let version = agent
        .napp_path
        .as_ref()
        .and_then(|p| {
            let manifest_path = std::path::PathBuf::from(p).join("manifest.json");
            std::fs::read_to_string(manifest_path).ok()
        })
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v["version"].as_str().map(|s| s.to_string()));

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

    // Derive needsSetup: true when any required input field is missing a value
    let needs_setup = if input_fields.is_empty() {
        false
    } else {
        let current_values: serde_json::Value =
            serde_json::from_str(&agent.input_values).unwrap_or_default();
        input_fields.iter().any(|field| {
            let required = field
                .get("required")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !required {
                return false;
            }
            let key = field.get("key").and_then(|v| v.as_str()).unwrap_or("");
            if key.is_empty() {
                return false;
            }
            match current_values.get(key) {
                None => true,
                Some(v) => v.is_null() || v.as_str().map_or(false, |s| s.is_empty()),
            }
        })
    };

    // Split agentMd into properties + body for the persona editor
    let (yaml_str, persona_body) =
        napp::agent::split_frontmatter(&agent.agent_md).unwrap_or_default();
    let persona_properties = parse_persona_properties(&yaml_str);

    // Extract model and skills from frontmatter for V2 frontend
    let frontmatter_val: serde_json::Value = if !agent.frontmatter.is_empty() {
        serde_json::from_str(&agent.frontmatter).unwrap_or_default()
    } else {
        serde_json::Value::Null
    };
    let model = frontmatter_val
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let skills: Vec<&str> = frontmatter_val
        .get("skills")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|s| s.as_str()).collect())
        .unwrap_or_default();

    // Return cached plugin auth status only — never block getAgent on shell commands.
    // The cache is populated lazily in the background on first plugin access.
    let plugins_needing_auth = get_cached_plugins_auth_status(&state).await;

    // Compute a human-readable display name (window title > first heading > name)
    let display_name = agent
        .app_window_config
        .as_ref()
        .and_then(|cfg_str| serde_json::from_str::<serde_json::Value>(cfg_str).ok())
        .and_then(|cfg| cfg.get("title").and_then(|t| t.as_str().map(|s| s.to_string())))
        .filter(|t| !t.is_empty())
        .or_else(|| {
            persona_body.lines().find_map(|line| {
                line.trim()
                    .strip_prefix("# ")
                    .map(|h| h.trim().to_string())
                    .filter(|h| !h.is_empty())
            })
        })
        .unwrap_or_else(|| agent.name.clone());

    // App agents: redact publisher IP (persona, skills content, frontmatter).
    // Only return what the frontend needs for display and configuration.
    let is_app = agent.is_app.unwrap_or(0) != 0;

    // Surface saved inputs at top level too (consistent with list_agents), so the
    // Configure form reads getAgent().inputValues directly instead of digging into
    // the nested agent object.
    let input_values_json = agent.input_values.clone();

    if is_app {
        Ok(Json(serde_json::json!({
            "agent": {
                "id": agent.id,
                "name": agent.name,
                "description": agent.description,
                "isApp": true,
                "isEnabled": agent.is_enabled,
                "kind": agent.kind,
                "appWindowConfig": agent.app_window_config,
                "inputValues": agent.input_values,
                "installedAt": agent.installed_at,
                "updatedAt": agent.updated_at,
                "pricingModel": agent.pricing_model,
                "pricingCost": agent.pricing_cost,
            },
            "displayName": display_name,
            "version": version,
            "inputFields": input_fields,
            "inputValues": input_values_json,
            "pluginsNeedingAuth": plugins_needing_auth,
            "needsSetup": needs_setup,
        })))
    } else {
        Ok(Json(serde_json::json!({
            "agent": agent,
            "displayName": display_name,
            "version": version,
            "inputFields": input_fields,
            "inputValues": input_values_json,
            "personaProperties": persona_properties,
            "persona": persona_body,
            "model": model,
            "skills": skills,
            "pluginsNeedingAuth": plugins_needing_auth,
            "needsSetup": needs_setup,
        })))
    }
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
        if fm.name.is_empty() {
            &existing.name
        } else {
            &fm.name
        }
    });
    let description = body["description"].as_str().unwrap_or_else(|| {
        if fm.description.is_empty() {
            &existing.description
        } else {
            &fm.description
        }
    });

    // Update agent_md frontmatter if name/description changed via body (not via agentMd)
    let final_agent_md = if body.get("agentMd").is_none()
        && (name != fm.name.as_str() || description != fm.description.as_str())
    {
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
            new_md.push_str(&format!(
                "pricing:\n  model: \"{}\"\n  cost: {}\n",
                p.model, p.cost
            ));
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
    let existing_fm: serde_json::Value =
        serde_json::from_str(&existing.frontmatter).unwrap_or_default();
    let workflows = existing_fm
        .get("workflows")
        .cloned()
        .unwrap_or(serde_json::json!({}));

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
    let soul = body["soul"].as_str();
    let rules = body["rules"].as_str();
    let handle = body["handle"].as_str();
    let color = body["color"].as_str();
    let loop_exposed = body["loopExposed"].as_bool();
    let exposure_changed =
        loop_exposed.is_some_and(|exposed| exposed != (existing.loop_exposed != 0));

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
            soul,
            rules,
            handle,
            color,
            loop_exposed,
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
            active.soul = updated.soul.clone();
            active.rules = updated.rules.clone();
        }
    }

    state.hub.broadcast(
        "agent_updated",
        serde_json::json!({ "agentId": id, "name": updated.name, "description": updated.description }),
    );

    // When loop exposure changed, sync this agent's loop presence live.
    // The primary ("assistant") is always present on the loop and is managed by
    // the gateway, never as a named secondary — so skip the register/deregister
    // call for it (the saved flag is harmless).
    if exposure_changed && id != "assistant" {
        let st = state.clone();
        let agent_name = updated.name.clone();
        let now_exposed = updated.loop_exposed != 0;
        tokio::spawn(async move {
            // The helpers derive the canonical bot-scoped handle
            // (`bot_<id8>_<slug>`) internally, matching reconcile.
            let result = if now_exposed {
                crate::codes::register_agent_in_loop(&st, &agent_name).await
            } else {
                crate::codes::deregister_agent_from_loop(&st, &agent_name).await
            };
            if let Err(e) = result {
                warn!(agent = %agent_name, exposed = now_exposed, error = %e, "failed to sync agent loop exposure");
            }
        });
    }

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

    // Reload the in-memory AgentLoader so the removal is reflected BEFORE the
    // frontend refreshes. list_agents() enumerates the loader (filesystem source
    // of truth); without this, the broadcast races ahead of the loader's own
    // filesystem-watch reload and the frontend refetches a stale roster that
    // still lists the just-deleted agent — the "requires a hard refresh" bug.
    // Mirrors the install path (codes.rs), which reloads before broadcasting.
    state.agent_loader.load_all().await;

    // Notify frontend — the roster is now consistent, so the refetch is correct.
    state.hub.broadcast(
        "agent_uninstalled",
        serde_json::json!({ "agentId": id, "name": agent.name }),
    );

    // Deregister agent from NeboAI (non-blocking, best-effort)
    {
        let st = state.clone();
        let agent_name = agent.name.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::codes::deregister_agent_from_loop(&st, &agent_name).await {
                warn!(agent = %agent_name, error = %e, "failed to deregister agent from loop");
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
        state.agent_workers.start_agent(&id, &agent.name, None).await;
    } else {
        state.agent_workers.stop_agent(&id).await;
    }

    Ok(Json(serde_json::json!({ "agent": agent })))
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
            napp::agent::AgentTrigger::Schedule { cron, .. } => {
                ("schedule", tools::PersonaTool::normalize_cron(cron))
            }
            napp::agent::AgentTrigger::Heartbeat { interval, window } => {
                let cfg = match window {
                    Some(w) => format!("{}|{}", interval, w),
                    None => interval.clone(),
                };
                ("heartbeat", cfg)
            }
            napp::agent::AgentTrigger::Event { sources } => ("event", sources.join(",")),
            napp::agent::AgentTrigger::Watch {
                plugin,
                command,
                event,
                restart_delay_secs,
            } => {
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
            napp::agent::AgentTrigger::Folder {
                path,
                extensions,
                recursive,
                debounce_secs,
            } => {
                let cfg = serde_json::json!({
                    "path": path,
                    "extensions": extensions,
                    "recursive": recursive,
                    "debounce_secs": debounce_secs
                });
                ("folder", cfg.to_string())
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

    // Register schedule cron rows from the bindings. Live trigger
    // registration (event subscriptions, heartbeat/watch/folder loops) is
    // owned by the AgentWorker — it registers everything when the agent is
    // activated, so installs never double-subscribe.
    if let Ok(bindings) = state.store.list_agent_workflows(agent_id) {
        workflow::triggers::register_agent_triggers(agent_id, &bindings, &state.store);
    }

    info!(
        agent = agent_id,
        bindings = config.workflows.len(),
        "processed agent bindings"
    );
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
                    soul: agent.soul.clone(),
                    rules: agent.rules.clone(),
    };
    state
        .agent_registry
        .write()
        .await
        .insert(id.clone(), active);
    state.agent_workers.start_agent(&id, &agent.name, None).await;

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

/// GET /agents/event-sources — every event source a trigger can subscribe to:
/// workflow emits (chaining) AND watch-plugin auto-emissions ({plugin}.{event}).
/// The builder offers these as suggestions — a typo'd source is a subscription
/// that silently never matches, so picking beats typing.
pub async fn list_event_sources(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    let emit_sources = state.store.list_emit_sources().map_err(to_error_response)?;

    let mut sources: Vec<serde_json::Value> = emit_sources
        .iter()
        .map(|es| {
            let slug = es.agent_name.to_lowercase().replace(' ', "-");
            let value = format!("{}.{}", slug, es.emit);
            let label = format!("{} > {}", es.agent_name, es.emit);
            serde_json::json!({
                "value": value,
                "label": label,
                "kind": "emit",
                "agentName": es.agent_name,
                "bindingName": es.binding_name,
                "description": es.description,
            })
        })
        .collect();

    // Watch bindings with an `event` auto-emit into the EventBus as
    // "{plugin}.{event}" (see AgentTrigger::Watch).
    if let Ok(watch_configs) = state.store.list_watch_trigger_configs() {
        for (agent_name, binding_name, config) in watch_configs {
            let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&config) else {
                continue;
            };
            let plugin = cfg.get("plugin").and_then(|v| v.as_str()).unwrap_or("");
            let event = cfg.get("event").and_then(|v| v.as_str()).unwrap_or("");
            if plugin.is_empty() || event.is_empty() {
                continue;
            }
            sources.push(serde_json::json!({
                "value": format!("{}.{}", plugin, event),
                "label": format!("{} watcher ({})", plugin, agent_name),
                "kind": "watch",
                "agentName": agent_name,
                "bindingName": binding_name,
                "description": serde_json::Value::Null,
            }));
        }
    }

    // Dedupe by value (several watchers can surface the same plugin event).
    let mut seen = std::collections::HashSet::new();
    sources.retain(|s| {
        let value = s.get("value").and_then(|v| v.as_str()).unwrap_or("").to_string();
        seen.insert(value)
    });

    Ok(Json(serde_json::json!({ "sources": sources })))
}

/// GET /agents/active — returns currently active agents from the AgentRegistry.
pub async fn list_active_agents(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    let registry = state.agent_registry.read().await;
    // `nextFireAt` is computed by evaluating each binding's cron in the
    // machine's local timezone — same as `scheduler::tick`. If we used UTC
    // here, the frontend's "Next: 7:00 AM" would diverge from when the
    // scheduler actually fires the job.
    let now = chrono::Local::now();

    let agents: Vec<serde_json::Value> = registry
        .values()
        .map(|agent| {
            // Fetch description from DB if available
            let description = state
                .store
                .get_agent(&agent.agent_id)
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
        })
        .collect();

    Ok(Json(serde_json::json!({
        "agents": agents,
        "count": agents.len(),
    })))
}

/// Compute the earliest next fire timestamp across all active bindings for an agent.
fn compute_next_fire(
    store: &db::Store,
    agent_id: &str,
    now: &chrono::DateTime<chrono::Local>,
) -> Option<i64> {
    let bindings = store.list_agent_workflows(agent_id).ok()?;
    let mut earliest: Option<i64> = None;
    let now_ts = now.timestamp();

    for binding in &bindings {
        if binding.is_active == 0 {
            continue;
        }

        let next_ts = match binding.trigger_type.as_str() {
            "schedule" => {
                // Normalize at read like the scheduler does — handles stale
                // 5-field expressions and numeric day-of-week in old rows.
                let normalized = tools::PersonaTool::normalize_cron(&binding.trigger_config);
                let schedule: cron::Schedule = match normalized.parse() {
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
                if interval_secs <= 0 {
                    // A zero/negative interval (e.g. "0m") would loop forever
                    // below — treat as unschedulable.
                    continue;
                }
                let last_fired = parse_last_fired(binding.last_fired.as_deref());
                let mut next = last_fired.timestamp() + interval_secs;

                // If next is in the past, advance to the next future interval
                if next <= now_ts {
                    let missed = (now_ts - next) / interval_secs + 1;
                    next += missed * interval_secs;
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

/// One interval parser for the whole system (types::timeutil) — the display
/// path here and the firing path in agent_worker must agree, or nextFireAt
/// shows times that never happen (e.g. "2h30m" used to display as 30m).
fn parse_interval_secs(s: &str) -> i64 {
    types::timeutil::parse_duration(s).as_secs() as i64
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

    let workflows = state
        .store
        .list_agent_workflows(&id)
        .map_err(to_error_response)?;

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
            let sources: Vec<&str> = trigger_config
                .split(',')
                .filter(|s| !s.is_empty())
                .collect();
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
        "folder" => {
            // trigger_config is JSON for folder triggers
            if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(trigger_config) {
                let mut t = serde_json::json!({ "type": "folder" });
                if let Some(obj) = cfg.as_object() {
                    for (k, v) in obj {
                        t[k] = v.clone();
                    }
                }
                t
            } else {
                serde_json::json!({ "type": "folder" })
            }
        }
        "manual" => serde_json::json!({ "type": "manual" }),
        _ => serde_json::json!({ "type": trigger_type }),
    }
}

/// Convert a cron expression to a human-readable schedule string.
/// Handles 5-field (minute hour dom month dow), 6-field, and
/// 7-field (second minute hour dom month dow year) formats.
fn cron_to_human_readable(cron: &str) -> String {
    let parts: Vec<&str> = cron.split_whitespace().collect();
    if parts.len() < 5 {
        return cron.to_string();
    }
    // 7-field: second minute hour dom month dow year
    // 6-field: second minute hour dom month dow  (normalize_cron sometimes)
    // 5-field: minute hour dom month dow
    let (minute, hour, _dom, _month, dow) = match parts.len() {
        7 | 6 => (parts[1], parts[2], parts[3], parts[4], parts[5]),
        _ => (parts[0], parts[1], parts[2], parts[3], parts[4]),
    };

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

/// POST /agents/{id}/check-update — check if a newer version is available on NeboAI.
pub async fn check_agent_update(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let agent = state
        .store
        .get_agent(&id)
        .map_err(to_error_response)?
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
            let local_version = agent
                .napp_path
                .as_ref()
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

/// POST /agents/{id}/apply-update — download and apply the latest version from NeboAI.
pub async fn apply_agent_update(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let agent = state
        .store
        .get_agent(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let kind = agent.kind.as_deref().unwrap_or("");
    if kind.is_empty() {
        return Err(to_error_response(types::NeboError::Validation(
            "Cannot update a user-created agent from marketplace".to_string(),
        )));
    }

    let api = crate::codes::build_api_client(&state).map_err(to_error_response)?;
    match tools::persist_agent_from_api(&api, &id, &agent.name, kind, &state.store).await {
        Ok(_) => {
            // Re-read updated agent
            let updated = state
                .store
                .get_agent(&id)
                .map_err(to_error_response)?
                .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

            // Update live registry
            if let Some(config) = if !updated.frontmatter.is_empty() {
                napp::agent::parse_agent_config(&updated.frontmatter).ok()
            } else {
                None
            } {
                let active = tools::ActiveAgent {
                    agent_id: id.clone(),
                    name: updated.name.clone(),
                    agent_md: updated.agent_md.clone(),
                    config: Some(config),
                    channel_id: None,
                    degraded: None,
                    soul: updated.soul.clone(),
                    rules: updated.rules.clone(),
                };
                state
                    .agent_registry
                    .write()
                    .await
                    .insert(id.clone(), active);
            }

            Ok(Json(serde_json::json!({
                "ok": true,
                "agent": updated,
            })))
        }
        Err(e) => Err(to_error_response(types::NeboError::Internal(format!(
            "Update failed: {}",
            e
        )))),
    }
}

/// POST /agents/{id}/reload — re-read AGENT.md + agent.json from filesystem and sync to DB.
pub async fn reload_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let agent = state
        .store
        .get_agent(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let agent_dir = agent
        .napp_path
        .as_ref()
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
        return Ok(Json(
            serde_json::json!({ "ok": true, "message": "Already in sync" }),
        ));
    }

    // Persist filesystem-owned content only; identity stays DB-owned.
    state
        .store
        .sync_agent_content(&id, &current_md, &current_fm)
        .map_err(to_error_response)?;

    // Re-register triggers if agent.json changed
    if changes.contains(&"agent.json") {
        if let Ok(config) = napp::agent::parse_agent_config(&current_fm) {
            let _ = state
                .store
                .delete_cron_jobs_by_prefix(&format!("agent-{}-", id));
            let _ = state.store.delete_agent_workflows(&id);
            process_agent_bindings(&id, &config, &state).await;
        }
    }

    // Update live registry
    if let Some(active) = state.agent_registry.write().await.get_mut(&id) {
        active.agent_md = current_md;
        active.config = napp::agent::parse_agent_config(&current_fm).ok();
    }

    // Re-sync live triggers with the reloaded bindings (worker owns them;
    // the awaited restart also clears the old config's subscriptions, which
    // this handler previously leaked).
    restart_agent_worker_if_active(&state, &id).await;

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
    let agent = state
        .store
        .get_agent(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    state.hub.broadcast(
        "agent_setup",
        serde_json::json!({
            "agentId": agent.id,
            "agentName": agent.name,
            "agentDescription": agent.description,
        }),
    );

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
    state
        .store
        .update_agent_input_values(&id, &values_str)
        .map_err(to_error_response)?;
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

    let stats = state
        .store
        .agent_workflow_stats(&id)
        .map_err(to_error_response)?;
    let errors = state
        .store
        .agent_recent_errors(&id, 5)
        .map_err(to_error_response)?;
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
    let t0 = std::time::Instant::now();

    state
        .store
        .get_agent(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    let t_agent = t0.elapsed();

    let wf_id = format!("agent:{}", id);
    let runs = state
        .store
        .list_workflow_runs(&wf_id, q.limit, q.offset)
        .map_err(to_error_response)?;
    let t_runs = t0.elapsed();

    let total = state
        .store
        .count_workflow_runs(&wf_id)
        .map_err(to_error_response)?;
    let t_count = t0.elapsed();

    let run_count = runs.len();
    let resp = serde_json::json!({ "runs": runs, "total": total });
    let t_serialize = t0.elapsed();

    tracing::info!(
        agent_id = %id,
        run_count = run_count,
        get_agent_ms = t_agent.as_millis(),
        list_runs_ms = (t_runs - t_agent).as_millis(),
        count_ms = (t_count - t_runs).as_millis(),
        serialize_ms = (t_serialize - t_count).as_millis(),
        total_ms = t_serialize.as_millis(),
        "list_agent_runs timing"
    );

    Ok(Json(resp))
}

/// POST /agents/{id}/workflows/{name}/run — manually trigger an agent workflow binding.
pub async fn run_agent_workflow(
    State(state): State<AppState>,
    Path((id, binding_name)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> HandlerResult<serde_json::Value> {
    let agent_rec = state
        .store
        .get_agent(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let config = napp::agent::parse_agent_config(&agent_rec.frontmatter)
        .map_err(|e| to_error_response(types::NeboError::Internal(format!("parse agent config: {}", e))))?;

    let binding = config
        .workflows
        .get(&binding_name)
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    if !binding.has_activities() {
        return Err(to_error_response(types::NeboError::Validation(
            "workflow binding has no activities".to_string(),
        )));
    }

    let def_json = binding.to_workflow_json(&binding_name);

    // Merge body inputs with binding defaults
    let mut inputs: serde_json::Value = serde_json::to_value(&binding.inputs).unwrap_or_default();
    if !body.is_empty() {
        if let Ok(parsed) = serde_json::from_slice::<serde_json::Value>(&body) {
            if let Some(extra) = parsed.get("inputs") {
                if let (Some(base), Some(extra_obj)) = (inputs.as_object_mut(), extra.as_object()) {
                    for (k, v) in extra_obj {
                        base.insert(k.clone(), v.clone());
                    }
                }
            }
        }
    }

    let emit_source = binding.emit.as_ref().map(|emit_name| {
        let slug = agent_rec.name.to_lowercase().replace(' ', "-");
        format!("{}.{}", slug, emit_name)
    });

    let run_id = state
        .workflow_manager
        .run_inline(
            def_json,
            inputs,
            "manual",
            Some(binding_name),
            &id,
            emit_source,
        )
        .await
        .map_err(|e| to_error_response(types::NeboError::Internal(e)))?;

    let run = state
        .store
        .get_workflow_run(&run_id)
        .map_err(to_error_response)?;

    Ok(Json(serde_json::json!({ "runId": run_id, "run": run })))
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
    state
        .store
        .set_agent_enabled(&id, true)
        .map_err(to_error_response)?;

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
                    soul: agent.soul.clone(),
                    rules: agent.rules.clone(),
    };

    state
        .agent_registry
        .write()
        .await
        .insert(agent_id.clone(), active);

    // Start autonomous agent worker (heartbeat, event, schedule triggers)
    state
        .agent_workers
        .start_agent(&agent_id, &agent.name, None)
        .await;

    // Launch sidecar binary for app agents using the shared .napp runtime.
    if agent.is_app.unwrap_or(0) != 0 {
        let old_lifecycle = {
            let mut lifecycles = state.app_lifecycles.write().await;
            lifecycles.remove(&agent_id)
        };
        if let Some(mut lifecycle) = old_lifecycle {
            if let Err(e) = lifecycle.shutdown().await {
                warn!(agent = %agent_id, error = %e, "failed to stop existing app sidecar");
            }
        }

        if let Some(tool_dir) = app_tool_dir(&agent) {
            let mut lifecycle = crate::app_lifecycle::AppLifecycle::new(
                agent_id.clone(),
                tool_dir,
                state.hub.clone(),
                state.tools.clone(),
                state.skill_loader.clone(),
                state.config.port,
            );
            match lifecycle.launch().await {
                Ok(()) => {
                    state
                        .app_lifecycles
                        .write()
                        .await
                        .insert(agent_id.clone(), lifecycle);
                }
                Err(e) => {
                    warn!(agent = %agent_id, error = %e, "failed to launch app sidecar");
                }
            }
        } else {
            warn!(agent = %agent_id, "app agent has no sidecar directory");
        }
    }

    // Register agent in the owner's personal loop (non-blocking)
    {
        let st = state.clone();
        let name = agent.name.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::codes::register_agent_in_loop(&st, &name).await {
                warn!(agent = %name, error = %e, "failed to register agent in loop");
            }
        });
    }

    state.hub.broadcast(
        "agent_activated",
        serde_json::json!({ "agentId": agent_id, "name": agent.name, "isApp": agent.is_app.unwrap_or(0) != 0 }),
    );

    Ok(Json(serde_json::json!({
        "agentId": agent_id,
        "name": agent.name,
        "status": "active",
        "isApp": agent.is_app.unwrap_or(0) != 0,
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

    let lifecycle = {
        let mut lifecycles = state.app_lifecycles.write().await;
        lifecycles.remove(&id)
    };
    if let Some(mut lifecycle) = lifecycle {
        if let Err(e) = lifecycle.shutdown().await {
            warn!(agent = %id, error = %e, "failed to stop app sidecar");
        }
    }

    let removed = state.agent_registry.write().await.remove(&id);
    match removed {
        Some(agent) => {
            // Deregister agent from the owner's personal loop (non-blocking)
            {
                let st = state.clone();
                let name = agent.name.clone();
                tokio::spawn(async move {
                    if let Err(e) = crate::codes::deregister_agent_from_loop(&st, &name).await {
                        warn!(agent = %name, error = %e, "failed to deregister agent from loop");
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

/// Replace the first `name:` line in an AGENT.md YAML frontmatter so a copy's
/// persona reflects its new name.
fn rewrite_agent_md_name(agent_md: &str, new_name: &str) -> String {
    if !agent_md.contains("name:") {
        return agent_md.to_string();
    }
    let mut result = String::new();
    let mut replaced = false;
    for line in agent_md.lines() {
        if !replaced && line.trim_start().starts_with("name:") {
            result.push_str(&format!("name: \"{new_name}\"\n"));
            replaced = true;
        } else {
            result.push_str(line);
            result.push('\n');
        }
    }
    result
}

/// POST /agents/{id}/duplicate — copy an agent (persona, skills, workflows, soul,
/// rules, entity_config) under a new name and auto-activate it. Does NOT copy
/// channel/account bindings, chats, or memories — the copy connects its own
/// accounts (see `needsAccountSetup` in the response). Body: `{ name?, color? }`.
pub async fn duplicate_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let source = state
        .store
        .get_agent(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let new_id = uuid::Uuid::new_v4().to_string();

    // Resolve the new name. The loader keys by name, so it must be unique. An
    // explicit name that collides is a 400 (let the user pick another); the default
    // "(Copy)" auto-suffixes so it always succeeds.
    let explicit = body
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let new_name = match explicit {
        Some(requested) => {
            if state
                .store
                .get_agent_by_name(requested)
                .map_err(to_error_response)?
                .is_some()
            {
                return Err(to_error_response(types::NeboError::Validation(format!(
                    "an agent named \"{requested}\" already exists"
                ))));
            }
            requested.to_string()
        }
        None => {
            let base = format!("{} (Copy)", source.name);
            let mut candidate = base.clone();
            let mut n = 2;
            while state
                .store
                .get_agent_by_name(&candidate)
                .map_err(to_error_response)?
                .is_some()
            {
                candidate = format!("{base} {n}");
                n += 1;
            }
            candidate
        }
    };
    let color = body.get("color").and_then(|v| v.as_str());

    let new_agent_md = rewrite_agent_md_name(&source.agent_md, &new_name);

    state
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

    // create_agent doesn't carry soul/rules/color — copy them from the source.
    // handle = None so the copy gets a fresh id-based loop handle (avoids a 409).
    let _ = state.store.update_agent(
        &new_id,
        &new_name,
        &source.description,
        &new_agent_md,
        &source.frontmatter,
        source.pricing_model.as_deref(),
        source.pricing_cost,
        source.soul.as_deref(),
        source.rules.as_deref(),
        None,
        color.or(source.color.as_deref()),
        None,
    );

    // Persist to user/agents/<name>/ (+ napp_path) so it loads and survives restart.
    write_user_agent_files(
        &state.store,
        &new_id,
        &new_name,
        &source.description,
        &new_agent_md,
        &source.frontmatter,
    );

    // Copy agent_workflow bindings from source.
    let source_workflows = state
        .store
        .list_agent_workflows(&id)
        .map_err(to_error_response)?;
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

    // Clone entity_config (heartbeat / permissions / model / personality / paths).
    if let Ok(Some(cfg)) = state.store.get_entity_config("agent", &id) {
        let patch = serde_json::json!({
            "heartbeatEnabled": cfg.heartbeat_enabled,
            "heartbeatIntervalMinutes": cfg.heartbeat_interval_minutes,
            "heartbeatContent": cfg.heartbeat_content,
            "heartbeatWindowStart": cfg.heartbeat_window_start,
            "heartbeatWindowEnd": cfg.heartbeat_window_end,
            "permissions": cfg.permissions,
            "resourceGrants": cfg.resource_grants,
            "modelPreference": cfg.model_preference,
            "personalitySnippet": cfg.personality_snippet,
            "allowedPaths": cfg.allowed_paths,
            "pinned": cfg.pinned,
            "multiChat": cfg.multi_chat,
        });
        let _ = state.store.upsert_entity_config("agent", &new_id, &patch);
    }

    // Reload the loader so the copy enumerates immediately (filesystem source).
    state.agent_loader.load_all().await;

    // Auto-activate (use the source's soul/rules — the fresh agent row has none yet
    // in this in-memory snapshot).
    let active = tools::ActiveAgent {
        agent_id: new_id.clone(),
        name: new_name.clone(),
        agent_md: new_agent_md.clone(),
        config: None,
        channel_id: None,
        degraded: None,
        soul: source.soul.clone(),
        rules: source.rules.clone(),
    };
    state
        .agent_registry
        .write()
        .await
        .insert(new_id.clone(), active);
    state
        .agent_workers
        .start_agent(&new_id, &new_name, None)
        .await;

    state.hub.broadcast(
        "agent_installed",
        serde_json::json!({ "agentId": &new_id, "name": &new_name }),
    );
    state.hub.broadcast(
        "agent_activated",
        serde_json::json!({ "agentId": &new_id, "name": &new_name }),
    );

    // Plugins the source had per-account profiles for — the copy starts with none,
    // so the UI prompts the user to connect the copy's own accounts.
    let needs_account_setup: Vec<String> = {
        let mut seen = std::collections::HashSet::new();
        state
            .store
            .list_all_plugin_account_profiles_for_agent(&id)
            .unwrap_or_default()
            .into_iter()
            .filter(|p| seen.insert(p.plugin_slug.clone()))
            .map(|p| p.plugin_slug)
            .collect()
    };

    Ok(Json(serde_json::json!({
        "agent": { "id": new_id, "name": new_name },
        "activated": true,
        "needsAccountSetup": needs_account_setup,
    })))
}

/// POST /agents/{id}/chat — send a message to an agent via the unified chat pipeline.
pub async fn chat_with_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let prompt = body["prompt"].as_str().unwrap_or("").to_string();
    if prompt.is_empty() {
        return Err(to_error_response(types::NeboError::Validation(
            "prompt is required".into(),
        )));
    }
    // Redact sensitive slash command arguments before storage
    let prompt = crate::redact::redact_sensitive_args(&prompt).unwrap_or(prompt);

    // Verify agent is active
    {
        let reg = state.agent_registry.read().await;
        if !reg.contains_key(&id) {
            return Err(to_error_response(types::NeboError::Validation(format!(
                "Agent '{}' is not active. Activate it first.",
                id
            ))));
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
        tool_scope: None, plan_mode: false,
        channel_ctx: None,
    };

    crate::chat_dispatch::run_chat(&state, config).await;

    Ok(Json(serde_json::json!({
        "sessionId": session_key,
        "agentId": id,
        "status": "dispatched",
    })))
}

// ── Workflow Binding CRUD ─────────────────────────────────────────────────────

/// Extract a heartbeat window as the canonical "HH:MM-HH:MM" string.
/// Accepts both the string form stored in agent.json and the
/// `{ start, end }` object form returned by `reconstruct_trigger`, so a
/// GET → PUT round-trip never silently drops the window.
fn heartbeat_window_str(trigger_config: &serde_json::Value) -> Option<String> {
    match trigger_config.get("window") {
        Some(serde_json::Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(serde_json::Value::Object(o)) => {
            let start = o.get("start").and_then(|v| v.as_str()).unwrap_or("");
            let end = o.get("end").and_then(|v| v.as_str()).unwrap_or("");
            if !start.is_empty() && !end.is_empty() {
                Some(format!("{}-{}", start, end))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Build trigger JSON for agent.json from flat (type, config) pair.
pub(crate) fn build_trigger_json(
    trigger_type: &str,
    trigger_config: &serde_json::Value,
) -> serde_json::Value {
    match trigger_type {
        "schedule" => {
            let cron = trigger_config
                .get("cron")
                .and_then(|v| v.as_str())
                .unwrap_or("0 * * * *");
            let mut t = serde_json::json!({ "type": "schedule", "cron": cron });
            if let Some(schedule) = trigger_config.get("schedule").and_then(|v| v.as_str()) {
                if !schedule.is_empty() {
                    t["schedule"] = serde_json::json!(schedule);
                }
            }
            t
        }
        "heartbeat" => {
            let interval = trigger_config
                .get("interval")
                .and_then(|v| v.as_str())
                .unwrap_or("30m");
            let mut t = serde_json::json!({ "type": "heartbeat", "interval": interval });
            if let Some(window) = heartbeat_window_str(trigger_config) {
                t["window"] = serde_json::json!(window);
            }
            t
        }
        "event" => {
            let sources: Vec<String> =
                if let Some(arr) = trigger_config.get("sources").and_then(|v| v.as_array()) {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                } else if let Some(s) = trigger_config.get("sources").and_then(|v| v.as_str()) {
                    s.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                } else {
                    vec![]
                };
            serde_json::json!({ "type": "event", "sources": sources })
        }
        "watch" => {
            // Pass every config key through (plugin, command, event, ...) —
            // the watch config shape is owned by the plugin.
            let mut t = serde_json::json!({ "type": "watch" });
            if let Some(obj) = trigger_config.as_object() {
                for (k, v) in obj {
                    if k != "type" {
                        t[k.as_str()] = v.clone();
                    }
                }
            }
            t
        }
        "folder" => {
            let mut t = serde_json::json!({ "type": "folder" });
            for key in &["path", "extensions", "recursive", "debounce_secs"] {
                if let Some(v) = trigger_config.get(*key) {
                    t[*key] = v.clone();
                }
            }
            t
        }
        _ => serde_json::json!({ "type": "manual" }),
    }
}

/// Flatten trigger config for DB storage (flat string).
pub(crate) fn flatten_trigger_config(
    trigger_type: &str,
    trigger_config: &serde_json::Value,
) -> String {
    match trigger_type {
        "schedule" => {
            let raw = trigger_config
                .get("cron")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            tools::PersonaTool::normalize_cron(raw)
        }
        "heartbeat" => {
            let interval = trigger_config
                .get("interval")
                .and_then(|v| v.as_str())
                .unwrap_or("30m");
            match heartbeat_window_str(trigger_config) {
                Some(w) => format!("{}|{}", interval, w),
                None => interval.to_string(),
            }
        }
        "event" => {
            if let Some(arr) = trigger_config.get("sources").and_then(|v| v.as_array()) {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            } else {
                trigger_config
                    .get("sources")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            }
        }
        // Watch and folder configs are stored as JSON — `reconstruct_trigger`
        // parses them back. A missing arm here meant any PUT on a watch
        // binding flattened its config to "" and killed the trigger.
        "watch" => trigger_config.to_string(),
        "folder" => trigger_config.to_string(),
        _ => String::new(),
    }
}

/// Write updated frontmatter back to filesystem agent.json if napp_path exists.
pub(crate) fn write_agent_json_to_fs(napp_path: &Option<String>, frontmatter: &serde_json::Value) {
    if let Some(path) = napp_path {
        let agent_json_path = std::path::Path::new(path).join("agent.json");
        if let Ok(pretty) = serde_json::to_string_pretty(frontmatter) {
            if let Err(e) = std::fs::write(&agent_json_path, &pretty) {
                warn!(path = %agent_json_path.display(), error = %e, "failed to write agent.json");
            }
        }
    }
}

/// Re-sync an active agent's live triggers by restarting its worker.
///
/// The AgentWorker is the SINGLE owner of live trigger registration — event
/// subscriptions, heartbeat/watch/folder loops, and cron rows (via
/// `register_agent_triggers` at worker start). After any binding mutation,
/// restarting the worker makes its registrations match the DB exactly:
/// `start_agent` awaits the old worker's stop (all unsubscribes complete)
/// before the new worker registers. Inactive agents have no worker and no
/// live triggers — their registrations happen on activation.
pub(crate) async fn restart_agent_worker_if_active(state: &AppState, agent_id: &str) {
    let name = match state.agent_registry.read().await.get(agent_id) {
        Some(active) => active.name.clone(),
        None => return,
    };
    state.agent_workers.start_agent(agent_id, &name, None).await;
}

/// POST /agents/{id}/workflows — create a new workflow binding.
pub async fn create_agent_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let agent = state
        .store
        .get_agent(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let binding_name = body["bindingName"].as_str().ok_or_else(|| {
        to_error_response(types::NeboError::Validation("bindingName required".into()))
    })?;
    let trigger_type = body["triggerType"].as_str().ok_or_else(|| {
        to_error_response(types::NeboError::Validation("triggerType required".into()))
    })?;
    let trigger_config = body
        .get("triggerConfig")
        .cloned()
        .unwrap_or(serde_json::json!({}));

    // Parse existing frontmatter
    let mut fm: serde_json::Value =
        serde_json::from_str(&agent.frontmatter).unwrap_or(serde_json::json!({}));

    // Check for conflict
    if fm
        .get("workflows")
        .and_then(|w| w.get(binding_name))
        .is_some()
    {
        return Err((
            StatusCode::CONFLICT,
            Json(types::api::ErrorResponse {
                error: format!("binding '{}' already exists", binding_name),
            }),
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
    if let Some(connections) = body.get("connections") {
        binding_val["connections"] = connections.clone();
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
    state
        .store
        .update_agent(
            &id,
            &agent.name,
            &agent.description,
            &agent.agent_md,
            &fm.to_string(),
            agent.pricing_model.as_deref(),
            agent.pricing_cost,
            None,
            None,
            None,
            None,
            None,
        )
        .map_err(to_error_response)?;

    // Upsert tracking row
    let trigger_config_flat = flatten_trigger_config(trigger_type, &trigger_config);
    let desc = body.get("description").and_then(|v| v.as_str());
    let inputs_json = body
        .get("inputs")
        .and_then(|v| serde_json::to_string(v).ok());
    let emit_val = body.get("emit").and_then(|v| v.as_str());
    let activities_json = body
        .get("activities")
        .and_then(|v| serde_json::to_string(v).ok());
    let connections_json = body
        .get("connections")
        .and_then(|v| serde_json::to_string(v).ok());
    state
        .store
        .upsert_agent_workflow(
            &id,
            binding_name,
            trigger_type,
            &trigger_config_flat,
            desc,
            inputs_json.as_deref(),
            emit_val,
            activities_json.as_deref(),
            connections_json.as_deref(),
        )
        .map_err(to_error_response)?;

    // The worker owns live trigger registration — restart it so the new
    // binding's trigger (heartbeat/watch/event/schedule) goes live now,
    // not at the next app restart.
    restart_agent_worker_if_active(&state, &id).await;

    // Write to filesystem
    write_agent_json_to_fs(&agent.napp_path, &fm);

    let workflows = state
        .store
        .list_agent_workflows(&id)
        .map_err(to_error_response)?;
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
    let agent = state
        .store
        .get_agent(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let mut fm: serde_json::Value =
        serde_json::from_str(&agent.frontmatter).unwrap_or(serde_json::json!({}));

    // Verify binding exists
    let existing_binding = fm
        .get("workflows")
        .and_then(|w| w.get(&binding_name))
        .cloned()
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    // Determine old trigger type for cleanup
    let old_trigger_type = existing_binding
        .get("trigger")
        .and_then(|t| t.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("manual");

    // Build updated binding — merge provided fields over existing
    let mut updated = existing_binding.clone();

    if let Some(trigger_type) = body.get("triggerType").and_then(|v| v.as_str()) {
        let trigger_config = body
            .get("triggerConfig")
            .cloned()
            .unwrap_or(serde_json::json!({}));
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
    if let Some(connections) = body.get("connections") {
        updated["connections"] = connections.clone();
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
    state
        .store
        .update_agent(
            &id,
            &agent.name,
            &agent.description,
            &agent.agent_md,
            &fm.to_string(),
            agent.pricing_model.as_deref(),
            agent.pricing_cost,
            None,
            None,
            None,
            None,
            None,
        )
        .map_err(to_error_response)?;

    // Determine new trigger info
    let new_trigger_type = body
        .get("triggerType")
        .and_then(|v| v.as_str())
        .unwrap_or(old_trigger_type);
    let trigger_config = body.get("triggerConfig").cloned().unwrap_or_else(|| {
        // Reconstruct from existing trigger
        existing_binding
            .get("trigger")
            .cloned()
            .unwrap_or(serde_json::json!({}))
    });
    let trigger_config_flat = flatten_trigger_config(new_trigger_type, &trigger_config);

    // Upsert tracking row
    let desc = fm["workflows"][&binding_name]
        .get("description")
        .and_then(|v| v.as_str());
    let inputs_json = fm["workflows"][&binding_name]
        .get("inputs")
        .and_then(|v| serde_json::to_string(v).ok());
    let emit_val = fm["workflows"][&binding_name]
        .get("emit")
        .and_then(|v| v.as_str());
    let activities_json = fm["workflows"][&binding_name]
        .get("activities")
        .and_then(|v| serde_json::to_string(v).ok());
    let connections_json = fm["workflows"][&binding_name]
        .get("connections")
        .and_then(|v| serde_json::to_string(v).ok());
    state
        .store
        .upsert_agent_workflow(
            &id,
            &binding_name,
            new_trigger_type,
            &trigger_config_flat,
            desc,
            inputs_json.as_deref(),
            emit_val,
            activities_json.as_deref(),
            connections_json.as_deref(),
        )
        .map_err(to_error_response)?;

    // If trigger type changed, clear the old cron row before re-sync.
    if body.get("triggerType").is_some() {
        workflow::triggers::unregister_single_agent_trigger(&id, &binding_name, &state.store);
    }

    // The worker owns live trigger registration — restart it so every PUT
    // (trigger change, description edit, new activities) takes effect now.
    restart_agent_worker_if_active(&state, &id).await;

    // Write to filesystem
    write_agent_json_to_fs(&agent.napp_path, &fm);

    let workflows = state
        .store
        .list_agent_workflows(&id)
        .map_err(to_error_response)?;
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
    state
        .store
        .get_agent(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let is_active = state
        .store
        .toggle_agent_workflow(&id, &binding_name)
        .map_err(to_error_response)?;

    if !is_active {
        // Remove the cron row — the worker won't prune rows for disabled
        // bindings, and a lingering row is misleading even though the
        // scheduler gates on is_active.
        workflow::triggers::unregister_single_agent_trigger(&id, &binding_name, &state.store);
    }

    // The worker owns live trigger registration — restart it so the toggle
    // takes effect immediately: ON starts heartbeat/watch/folder loops and
    // event subs; OFF tears them down (worker start skips inactive bindings).
    restart_agent_worker_if_active(&state, &id).await;

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
    let agent = state
        .store
        .get_agent(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    // Remove from frontmatter
    let mut fm: serde_json::Value =
        serde_json::from_str(&agent.frontmatter).unwrap_or(serde_json::json!({}));
    if let Some(workflows) = fm.get_mut("workflows").and_then(|w| w.as_object_mut()) {
        workflows.remove(&binding_name);
    }

    // Update agent in DB
    state
        .store
        .update_agent(
            &id,
            &agent.name,
            &agent.description,
            &agent.agent_md,
            &fm.to_string(),
            agent.pricing_model.as_deref(),
            agent.pricing_cost,
            None,
            None,
            None,
            None,
            None,
        )
        .map_err(to_error_response)?;

    // Delete tracking row
    state
        .store
        .delete_single_agent_workflow(&id, &binding_name)
        .map_err(to_error_response)?;

    // Remove the cron row, then restart the worker — it owns live trigger
    // registration, and the restart tears down the deleted binding's
    // heartbeat/watch/folder loops and event subscriptions immediately.
    workflow::triggers::unregister_single_agent_trigger(&id, &binding_name, &state.store);
    restart_agent_worker_if_active(&state, &id).await;

    // Write to filesystem
    write_agent_json_to_fs(&agent.napp_path, &fm);

    Ok(Json(serde_json::json!({
        "message": format!("Binding '{}' deleted", binding_name),
    })))
}

/// GET /agents/{id}/surfaces — Return A2UI replay messages for active surfaces.
pub async fn get_agent_surfaces(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
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

/// Return cached plugin auth status — never spawns subprocesses.
/// Returns whatever is in the auth cache right now (may be empty on cold start).
/// The cache gets populated lazily when plugins are actually used.
async fn get_cached_plugins_auth_status(state: &AppState) -> Vec<serde_json::Value> {
    let needing_auth = state.plugin_store.plugins_needing_auth().await;
    needing_auth
        .into_iter()
        .map(|(slug, auth)| {
            serde_json::json!({
                "slug": slug,
                "label": auth.label,
                "description": auth.description,
            })
        })
        .collect()
}

// ── Agent Multi-Chat ─────────────────────────────────────────────────────────

/// GET /api/v1/agents/{id}/chats — list all chats for an agent.
pub async fn list_agent_chats(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let session_prefix = format!("agent:{}:", id);

    // Resolve active chat_id from the legacy web session (if it exists).
    let legacy_session_key = agent::keyparser::build_agent_session_key(&id, "web");
    let active_chat_id = state
        .runner
        .sessions()
        .resolve_session_id_by_key(&legacy_session_key)
        .ok()
        .map(|sid| state.runner.sessions().active_chat_id(&sid))
        .unwrap_or_default();

    // Prefix query: catches both legacy `agent:<id>:web` and new `agent:<id>:thread:<uuid>`.
    let mut enriched_chats = state
        .store
        .list_chats_by_session_enriched(&session_prefix)
        .unwrap_or_default();

    // Backfill: legacy agent chats store messages under the session key as chat_id
    // but have no `chats` row. If we found no chats but messages exist, create the row.
    if enriched_chats.is_empty() {
        let legacy_chat_id = if active_chat_id.is_empty() {
            &legacy_session_key
        } else {
            &active_chat_id
        };
        let msg_count = state.store.count_chat_messages(legacy_chat_id).unwrap_or(0);
        if msg_count > 0 {
            if let Ok(chat) = state.store.create_chat_for_session(
                legacy_chat_id,
                &legacy_session_key,
                "Chat 1",
                None,
            ) {
                enriched_chats.push((chat, msg_count, String::new()));
            }
        }
    }

    // Format response
    let now = chrono::Utc::now().timestamp();
    let enriched: Vec<serde_json::Value> = enriched_chats
        .iter()
        .map(|(chat, msg_count, last_content)| {
            let clean = strip_to_plain(last_content);
            let preview = if clean.chars().count() > 120 {
                format!("{}...", clean.chars().take(120).collect::<String>())
            } else {
                clean
            };
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
        })
        .collect();

    let total = enriched.len();
    Ok(Json(serde_json::json!({
        "chats": enriched,
        "activeChatId": active_chat_id,
        "total": total,
    })))
}

/// Strip HTML tags and markdown markers to a plain-text thread-list preview snippet.
/// (Hidden system-reminder messages are already excluded at the query layer; this
/// cleans markdown/HTML in normal assistant/user content like `## Heading`, `**bold**`.)
fn strip_to_plain(input: &str) -> String {
    use std::sync::LazyLock;
    static TAG: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"<[^>]+>").unwrap());
    static LINK: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\[([^\]]+)\]\([^)]*\)").unwrap());
    static MD: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"[*_`#>~]+").unwrap());
    static WS: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\s+").unwrap());

    let s = TAG.replace_all(input, " "); // strip HTML/XML tags (incl. any <system-reminder>)
    let s = LINK.replace_all(&s, "$1"); // [text](url) -> text
    let s = MD.replace_all(&s, ""); // strip markdown markers * _ ` # > ~
    let s = s.replace("- ", " "); // strip list-item dashes
    WS.replace_all(&s, " ").trim().to_string() // collapse whitespace/newlines
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

/// POST /api/v1/agents/{id}/chats — create a new chat under a per-thread session.
pub async fn create_new_agent_chat(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let new_chat_id = uuid::Uuid::new_v4().to_string();
    let session_key = format!("agent:{}:thread:{}", id, new_chat_id);

    // Creates a new session with active_chat_id = new_chat_id (via extract_chat_id_from_key).
    let _session = state
        .runner
        .sessions()
        .get_or_create(&session_key, "")
        .map_err(to_error_response)?;

    // Create the chat row linked to this session.
    let chat = state
        .store
        .create_chat_for_session(&new_chat_id, &session_key, "New Chat", None)
        .map_err(to_error_response)?;

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

    let session_id = state
        .runner
        .sessions()
        .resolve_session_id_by_key(&session_key)
        .map_err(to_error_response)?;

    state
        .runner
        .sessions()
        .set_active_chat(&session_id, &chat_id)
        .map_err(to_error_response)?;

    let mut messages = state
        .store
        .get_chat_messages_budgeted(&chat_id, 12000, None)
        .unwrap_or_default();
    super::chat::build_message_metadata(&mut messages);
    let total = state
        .store
        .count_chat_messages(&chat_id)
        .unwrap_or(messages.len() as i64);

    Ok(Json(serde_json::json!({
        "chatId": chat_id,
        "messages": messages,
        "totalMessages": total,
        "sessionKey": session_key,
    })))
}

// ── Channel Bindings ─────────────────────────────────────────────────

/// GET /agents/{id}/channels — list channel plugins and their binding state.
///
/// Returns all installed plugins that declare a `channel` capability, along
/// with whether each is enabled for this agent.
pub async fn list_agent_channels(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let bindings = state
        .store
        .list_channel_bindings_for_agent(&agent_id)
        .map_err(to_error_response)?;

    // Discover all plugins with channel capability
    let installed = state.plugin_store.list_installed();
    let mut channels = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (slug, _version, _path, _source) in &installed {
        if !seen.insert(slug.clone()) {
            continue;
        }
        if let Some(ch) = state.plugin_store.get_channel_def(slug) {
            let binding = bindings.iter().find(|b| b.plugin_slug == *slug);
            let enabled = binding.map(|b| b.is_enabled).unwrap_or(false);
            let has_agent_config = binding
                .map(|b| !b.config.is_empty())
                .unwrap_or(false);

            let auth_info = state.plugin_store.get_auth_info(slug);
            let needs_auth = auth_info.is_some();
            // Auth status is computed PER AGENT. For env-type channel auth
            // (e.g. Slack bot tokens), each agent runs its own bot identity
            // with its own credentials — so an agent is authenticated only if
            // ITS binding holds every required token. No global fallback:
            // sharing a sibling agent's auth status is what made a tokenless
            // binding look "connected" and let the wizard skip the tokens.
            // Only non-env auth (machine-global OAuth like gws) falls back to
            // the plugin-level status check.
            let authenticated = match &auth_info {
                Some((_, a)) if a.auth_type == "env" => {
                    !a.env.is_empty()
                        && a.env.keys().all(|k| {
                            binding
                                .map(|b| b.config.get(k).is_some_and(|v| !v.is_empty()))
                                .unwrap_or(false)
                        })
                }
                Some(_) => state.plugin_store.check_auth_lazy(slug).await,
                None => true,
            };

            let (auth_label, auth_env_keys, auth_help) = match &auth_info {
                Some((_, a)) => (
                    a.label.clone(),
                    a.env.keys().cloned().collect::<Vec<_>>(),
                    a.help.as_ref().map(|h| serde_json::json!({
                        "url": h.url,
                        "urlLabel": h.url_label,
                        "text": h.text,
                    })),
                ),
                None => (String::new(), Vec::new(), None),
            };
            // Surface the plugin's setup wizard (if declared) so the channel
            // connect modal can render the guided flow instead of the bare
            // token form. Same ArtifactSetup the Settings → Plugins page uses.
            let setup = state
                .plugin_store
                .get_manifest(slug)
                .and_then(|m| m.setup)
                .map(|s| serde_json::to_value(s).unwrap_or(serde_json::Value::Null));
            // Non-secret saved values (e.g. bot name/description from a prior
            // wizard run) so the wizard can pre-fill. Exclude the auth env
            // keys — those are secrets we never return to the client.
            let saved_values: std::collections::HashMap<String, String> = binding
                .map(|b| {
                    b.config
                        .iter()
                        .filter(|(k, _)| !auth_env_keys.contains(k))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect()
                })
                .unwrap_or_default();
            channels.push(serde_json::json!({
                "pluginSlug": slug,
                "name": if ch.name.is_empty() { slug.clone() } else { ch.name },
                "description": ch.description,
                "enabled": enabled,
                "authenticated": authenticated,
                "needsAuth": needs_auth,
                "authLabel": auth_label,
                "authEnvKeys": auth_env_keys,
                "authHelp": auth_help,
                "hasAgentConfig": has_agent_config,
                "setup": setup,
                "savedValues": saved_values,
            }));
        }
    }

    Ok(Json(serde_json::json!({ "channels": channels })))
}

/// POST /agents/{id}/channels/{plugin_slug}/enable — enable a channel for this agent.
pub async fn enable_agent_channel(
    State(state): State<AppState>,
    Path((agent_id, plugin_slug)): Path<(String, String)>,
) -> HandlerResult<serde_json::Value> {
    // Verify plugin has channel capability
    if state.plugin_store.get_channel_def(&plugin_slug).is_none() {
        return Err(to_error_response(types::NeboError::Validation(format!(
            "plugin '{}' has no channel capability",
            plugin_slug
        ))));
    }

    state
        .store
        .enable_channel_binding(&agent_id, &plugin_slug)
        .map_err(to_error_response)?;

    // Restart agent worker so the channel loop starts
    if let Ok(Some(agent)) = state.store.get_agent(&agent_id) {
        let cfg = napp::agent::parse_agent_config(&agent.frontmatter).ok();
        state
            .agent_workers
            .start_agent(&agent_id, &agent.name, cfg)
            .await;
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /agents/{id}/channels/{plugin_slug}/disable — disable a channel for this agent.
pub async fn disable_agent_channel(
    State(state): State<AppState>,
    Path((agent_id, plugin_slug)): Path<(String, String)>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .disable_channel_binding(&agent_id, &plugin_slug)
        .map_err(to_error_response)?;

    // Restart agent worker so the channel loop stops
    if let Ok(Some(agent)) = state.store.get_agent(&agent_id) {
        let cfg = napp::agent::parse_agent_config(&agent.frontmatter).ok();
        state
            .agent_workers
            .start_agent(&agent_id, &agent.name, cfg)
            .await;
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// PUT /agents/{id}/channels/{plugin_slug}/config — save per-agent channel credentials.
///
/// Each agent can have its own bot token / app token so it appears as a
/// separate identity on Slack, Discord, etc.
pub async fn set_agent_channel_config(
    State(state): State<AppState>,
    Path((agent_id, plugin_slug)): Path<(String, String)>,
    Json(body): Json<std::collections::HashMap<String, String>>,
) -> HandlerResult<serde_json::Value> {
    // Verify plugin has channel capability
    if state.plugin_store.get_channel_def(&plugin_slug).is_none() {
        return Err(to_error_response(types::NeboError::Validation(format!(
            "plugin '{}' has no channel capability",
            plugin_slug
        ))));
    }

    // Only allow keys that are declared in the plugin's auth.env
    let allowed_keys: std::collections::HashSet<String> = state
        .plugin_store
        .get_auth_info(&plugin_slug)
        .map(|(_, a)| a.env.keys().cloned().collect())
        .unwrap_or_default();

    let filtered: std::collections::HashMap<String, String> = body
        .into_iter()
        .filter(|(k, v)| allowed_keys.contains(k) && !v.is_empty())
        .collect();

    state
        .store
        .set_channel_binding_config(&agent_id, &plugin_slug, &filtered)
        .map_err(to_error_response)?;

    // Restart agent worker so the bridge picks up the new credentials
    if let Ok(Some(agent)) = state.store.get_agent(&agent_id) {
        let cfg = napp::agent::parse_agent_config(&agent.frontmatter).ok();
        state
            .agent_workers
            .start_agent(&agent_id, &agent.name, cfg)
            .await;
    }

    info!(agent = %agent_id, plugin = %plugin_slug, keys = filtered.len(), "saved per-agent channel config");
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /agents/{id}/workflow/chat — open a workflow builder help chat session.
///
/// Creates a dedicated session seeded with the agent's workflow configs as context
/// so the AI is an expert on the specific workflows, activities, and steps.
/// The frontend passes the current workflow state; we serialize it into the system prompt.
pub async fn start_workflow_chat(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    // Verify agent exists
    let agent = state
        .store
        .get_agent(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let agent_name = &agent.name;

    // The frontend sends the full workflow state so the AI has real-time context
    let workflows_json = body
        .get("workflows")
        .cloned()
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

    let selected_workflow = body
        .get("selectedWorkflow")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let selected_activity = body
        .get("selectedActivity")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Build rich system context
    let workflows_pretty =
        serde_json::to_string_pretty(&workflows_json).unwrap_or_else(|_| "{}".to_string());

    let mut system_parts = vec![format!(
        "You are the Architect — the workflow builder assistant for the agent \"{agent_name}\". \
         You can SEE the user's current draft (below) and you can EDIT it by emitting structured ops.\n\n\
         ## Editing the draft\n\
         When the user asks for a change, include EXACTLY ONE fenced block in your reply:\n\
         ```workflow-ops\n\
         {{\"ops\": [ ... ]}}\n\
         ```\n\
         The builder applies the ops to the user's open DRAFT (one undo step; nothing is saved \
         until they press Save). The block is stripped from your message before display, so ALSO \
         describe what you changed in one short sentence. Never emit ops for questions — answer in prose.\n\n\
         ## Ops\n\
         - {{\"op\":\"add_activity\",\"workflow\":NAME,\"activity\":{{id,type,intent,skills?,steps?,params?}},\"after\":ID|\"__trigger__\"|null,\"branchLabel\"?:LABEL}}\n\
         - {{\"op\":\"update_activity\",\"workflow\":NAME,\"id\":ID,\"set\":{{...partial activity}}}}\n\
         - {{\"op\":\"remove_activity\",\"workflow\":NAME,\"id\":ID}}\n\
         - {{\"op\":\"connect\",\"workflow\":NAME,\"from\":ID|\"__trigger__\",\"to\":ID|\"__emit__\",\"label\"?:LABEL}}\n\
         - {{\"op\":\"disconnect\",\"workflow\":NAME,\"from\":ID,\"to\":ID}}\n\
         - {{\"op\":\"set_trigger\",\"workflow\":NAME,\"trigger\":{{type:\"schedule\"|\"heartbeat\"|\"event\"|\"manual\",schedule?,cron?,interval?,window?,event?}}}}\n\
         - {{\"op\":\"set_emit\",\"workflow\":NAME,\"emit\":EVENT|null}} / {{\"op\":\"set_description\",...}}\n\
         - {{\"op\":\"create_workflow\",\"name\":NAME,\"workflow\"?:{{...}}}} / {{\"op\":\"delete_workflow\",\"workflow\":NAME}} / {{\"op\":\"rename_workflow\",\"from\":A,\"to\":B}}\n\n\
         ## Activity types (the only valid `type` values)\n\
         custom, research (params: depth, sources), email (params: to, subject), notify, \
         code (params: language, code), http (params: method, url, headers, body — runs deterministically, no AI), \
         transform, condition, loop, wait (params: duration e.g. \"5m\"), \
         agent (params: agentId — delegation), connector (params: serverId, tool, input — MCP)\n\n\
         ## Execution semantics (the engine owns ALL control flow — deterministic, repeatable)\n\
         - Activities run sequentially along connections (__trigger__ → ... → __emit__); a node with \
           multiple outgoing edges runs its branches IN PARALLEL; a join waits for all active branches.\n\
         - condition REQUIRES params.expression + params.mode (expression|contains|exists|regex) and \
           routes via edges labeled \"True\"/\"False\". Routing is never decided by the AI.\n\
         - loop REQUIRES params.source (data path, e.g. \"inputs.items\") and uses edges labeled \
           \"Each item\" (body) and \"Done\". The AI lives INSIDE activities (intent + steps), never in routing.\n\n\
         ## Example\n\
         User: \"notify me when an urgent email arrives\"\n\
         ```workflow-ops\n\
         {{\"ops\":[\n\
           {{\"op\":\"add_activity\",\"workflow\":\"triage\",\"activity\":{{\"id\":\"urgent-check\",\"type\":\"condition\",\"params\":{{\"expression\":\"subject contains urgent\",\"mode\":\"contains\"}}}},\"after\":\"classify\"}},\n\
           {{\"op\":\"add_activity\",\"workflow\":\"triage\",\"activity\":{{\"id\":\"ping-owner\",\"type\":\"notify\",\"intent\":\"Notify the owner about the urgent email\"}},\"after\":null}},\n\
           {{\"op\":\"connect\",\"workflow\":\"triage\",\"from\":\"urgent-check\",\"to\":\"ping-owner\",\"label\":\"True\"}}\n\
         ]}}\n\
         ```\n\n\
         ## Rules\n\
         - Reference ONLY workflow names and activity ids that exist in the draft below (or ones you create in the same batch).\n\
         - New activity ids: short kebab-case, unique within the workflow.\n\
         - Be concise. One sentence on what you changed; no restating the JSON.\n\
         - If the user asks something outside workflow building, redirect them to the main chat."
    )];

    system_parts.push(format!(
        "## Current Workflows for \"{agent_name}\"\n\n```json\n{workflows_pretty}\n```"
    ));

    if !selected_workflow.is_empty() {
        system_parts.push(format!(
            "## Currently Selected\nWorkflow: **{selected_workflow}**"
        ));
        if !selected_activity.is_empty() {
            system_parts.push(format!("Activity: **{selected_activity}**"));
        }
    }

    let system_context = system_parts.join("\n\n");

    // Create a dedicated help session scoped to this agent + workflow builder
    let session_key =
        agent::keyparser::build_agent_session_key(&id, "help:workflow");

    let session = state
        .runner
        .sessions()
        .get_or_create(&session_key, "")
        .map_err(to_error_response)?;

    // Refresh mode: the builder calls this before each send when the draft
    // changed, so the Architect always sees CURRENT state. Replace only the
    // seeded system message — the conversation is preserved. Falls through
    // to a full seed if the session has no system message yet.
    let refresh = body
        .get("refresh")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if refresh {
        if let Ok(messages) = state.runner.sessions().get_messages(&session.id) {
            if let Some(sys) = messages.iter().find(|m| m.role == "system") {
                state
                    .store
                    .update_chat_message_content(&sys.id, &system_context, None)
                    .map_err(to_error_response)?;
                return Ok(Json(serde_json::json!({
                    "sessionKey": session_key,
                    "agentId": id,
                })));
            }
        }
    }

    // Full seed (builder open / first visit): clear old messages and inject
    // fresh context so the AI sees the latest workflow state.
    let _ = state.runner.sessions().clear_current_messages(&session.id);

    let _ = state.runner.sessions().append_message(
        &session.id,
        "system",
        &system_context,
        None,
        None,
        None,
    );

    let greeting = format!(
        "I'm the **Architect** — your workflow builder assistant for **{agent_name}**.\n\n\
         I can see your current workflows and help you modify them. Try:\n\
         - \"Add an email notification step after the review\"\n\
         - \"Change the trigger to run every 30 minutes\"\n\
         - \"What does the {} workflow do?\"\n\
         - \"How can I chain two workflows together?\"",
        if selected_workflow.is_empty() {
            "morning-brief".to_string()
        } else {
            selected_workflow.to_string()
        }
    );

    let _ = state.runner.sessions().append_message(
        &session.id,
        "assistant",
        &greeting,
        None,
        None,
        None,
    );

    Ok(Json(serde_json::json!({
        "sessionKey": session_key,
        "agentId": id,
    })))
}

#[derive(Debug, Deserialize)]
pub struct HandleAvailableQuery {
    pub handle: String,
}

#[derive(serde::Serialize)]
pub struct HandleAvailableResponse {
    pub available: bool,
}

/// GET /api/v1/agent/handle-available?handle=bot_xxx
///
/// Proxies the global handle-availability check to NeboAI. The handle is the
/// routing identity (`bot_<chosen>`) and is independent of any display name.
/// This bot is excluded server-side so its own current handle is never reported
/// as taken.
pub async fn handle_available(
    State(state): State<AppState>,
    Query(q): Query<HandleAvailableQuery>,
) -> HandlerResult<HandleAvailableResponse> {
    let api = crate::codes::build_api_client(&state).map_err(to_error_response)?;
    let available = api
        .handle_available(&q.handle)
        .await
        .map_err(|e| to_error_response(types::NeboError::Internal(e.to_string())))?;
    Ok(Json(HandleAvailableResponse { available }))
}
