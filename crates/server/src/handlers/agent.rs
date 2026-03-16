use axum::extract::{Path, Query, State};
use axum::response::Json;
use serde::Deserialize;

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

/// GET /api/v1/agent/sessions
pub async fn list_sessions(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> HandlerResult<serde_json::Value> {
    let sessions = state.store.list_sessions(q.limit, q.offset).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"sessions": sessions})))
}

/// DELETE /api/v1/agent/sessions/:id
pub async fn delete_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    state.store.delete_session(&id).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"success": true})))
}

/// GET /api/v1/agent/sessions/:id/messages
pub async fn get_session_messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    // Session has a scope/scope_id that maps to a chat
    let session = state
        .store
        .get_session(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    // Messages are stored under session.name (the session_key / chat_id).
    // Fall back to scope_id, then session id for legacy data.
    let chat_id = session
        .name
        .as_deref()
        .filter(|n| !n.is_empty())
        .or(session.scope_id.as_deref())
        .unwrap_or(&id);
    let messages = state.store.get_chat_messages(chat_id).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"messages": messages})))
}

/// GET /api/v1/agent/settings
pub async fn get_settings(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let settings = state.store.get_settings().map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"settings": settings})))
}

/// PUT /api/v1/agent/settings
pub async fn update_settings(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .update_settings(
            body["autonomousMode"].as_bool(),
            body["autoApproveRead"].as_bool(),
            body["autoApproveWrite"].as_bool(),
            body["autoApproveBash"].as_bool(),
            body["heartbeatIntervalMinutes"].as_i64(),
            body["commEnabled"].as_bool(),
            body["commPlugin"].as_str(),
            body["developerMode"].as_bool(),
            body["autoUpdate"].as_bool(),
        )
        .map_err(to_error_response)?;

    let settings = state.store.get_settings().map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"settings": settings})))
}

/// GET /api/v1/agent/profile
pub async fn get_profile(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    // Best-effort: ensure default profile row exists before reading
    let _ = state.store.ensure_agent_profile();
    let profile = state.store.get_agent_profile().map_err(to_error_response)?;
    Ok(Json(serde_json::json!(profile)))
}

/// PUT /api/v1/agent/profile
pub async fn update_profile(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    // Best-effort: ensure default profile row exists before updating
    let _ = state.store.ensure_agent_profile();
    state
        .store
        .update_agent_profile(
            body["name"].as_str(),
            body["personalityPreset"].as_str(),
            body["customPersonality"].as_str(),
            body["voiceStyle"].as_str(),
            body["responseLength"].as_str(),
            body["emojiUsage"].as_str(),
            body["formality"].as_str(),
            body["proactivity"].as_str(),
            body["emoji"].as_str(),
            body["creature"].as_str(),
            body["vibe"].as_str(),
            body["role"].as_str(),
            body["avatar"].as_str(),
            body["agentRules"].as_str(),
            body["toolNotes"].as_str(),
            body["quietHoursStart"].as_str(),
            body["quietHoursEnd"].as_str(),
        )
        .map_err(to_error_response)?;

    let profile = state.store.get_agent_profile().map_err(to_error_response)?;
    Ok(Json(serde_json::json!(profile)))
}

/// GET /api/v1/agent/status
pub async fn get_status(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let provider_count = state.runner.provider_count();
    let tool_names = state.tools.get_tool_names().await;

    // Get real task counts from the DB
    let active_tasks = state.store.get_pending_tasks_by_status("in_progress")
        .map(|t| t.len())
        .unwrap_or(0);
    let queued_tasks = state.store.get_pending_tasks_by_status("pending")
        .map(|t| t.len())
        .unwrap_or(0);

    Ok(Json(serde_json::json!({
        "status": if provider_count > 0 { "ready" } else { "no_providers" },
        "activeTasks": active_tasks,
        "queuedTasks": queued_tasks,
        "providers": provider_count,
        "tools": tool_names.len(),
    })))
}

/// GET /api/v1/agent/system-info
pub async fn get_system_info() -> HandlerResult<serde_json::Value> {
    Ok(Json(serde_json::json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "version": env!("CARGO_PKG_VERSION"),
    })))
}

/// GET /api/v1/agent/personality-presets
pub async fn list_personality_presets() -> HandlerResult<serde_json::Value> {
    Ok(Json(serde_json::json!({
        "presets": [
            {
                "id": "balanced",
                "name": "Balanced",
                "description": "Helpful, clear, and well-rounded",
                "voiceStyle": "conversational",
                "responseLength": "moderate",
                "emojiUsage": "occasional",
                "formality": "neutral",
                "proactivity": "moderate"
            },
            {
                "id": "creative",
                "name": "Creative",
                "description": "Imaginative, expressive, and playful",
                "voiceStyle": "expressive",
                "responseLength": "detailed",
                "emojiUsage": "frequent",
                "formality": "casual",
                "proactivity": "high"
            },
            {
                "id": "precise",
                "name": "Precise",
                "description": "Exact, technical, and methodical",
                "voiceStyle": "technical",
                "responseLength": "concise",
                "emojiUsage": "none",
                "formality": "formal",
                "proactivity": "low"
            },
            {
                "id": "casual",
                "name": "Casual",
                "description": "Relaxed, friendly, and approachable",
                "voiceStyle": "casual",
                "responseLength": "brief",
                "emojiUsage": "frequent",
                "formality": "informal",
                "proactivity": "moderate"
            },
            {
                "id": "professional",
                "name": "Professional",
                "description": "Formal, structured, and business-oriented",
                "voiceStyle": "professional",
                "responseLength": "thorough",
                "emojiUsage": "none",
                "formality": "formal",
                "proactivity": "moderate"
            },
            {
                "id": "concise",
                "name": "Concise",
                "description": "Minimal, direct, and to the point",
                "voiceStyle": "terse",
                "responseLength": "minimal",
                "emojiUsage": "none",
                "formality": "neutral",
                "proactivity": "low"
            },
            {
                "id": "mentor",
                "name": "Mentor",
                "description": "Patient, educational, and encouraging",
                "voiceStyle": "educational",
                "responseLength": "detailed",
                "emojiUsage": "occasional",
                "formality": "neutral",
                "proactivity": "high"
            }
        ]
    })))
}

/// GET /api/v1/agent/channels/:channelId/messages
pub async fn get_channel_messages(
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    // Channel messages map to chat messages in the channel's session
    let session = state
        .store
        .get_session_by_scope("channel", &channel_id)
        .map_err(to_error_response)?;

    let messages = match session {
        Some(s) => {
            let chat_id = s.name.as_deref().filter(|n| !n.is_empty())
                .or(s.scope_id.as_deref())
                .unwrap_or(&s.id);
            state.store.get_chat_messages(chat_id).unwrap_or_default()
        }
        None => vec![],
    };

    Ok(Json(serde_json::json!({"messages": messages})))
}

/// POST /api/v1/agent/channels/:channelId/send
pub async fn send_channel_message(
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let content = body["content"]
        .as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("content required".into())))?;

    // Get or create session for channel
    let session_id = uuid::Uuid::new_v4().to_string();
    let session = state
        .store
        .get_or_create_scoped_session(&session_id, &channel_id, "channel", &channel_id, None)
        .map_err(to_error_response)?;

    let chat_id = session.scope_id.as_deref().unwrap_or(&session.id);
    let msg_id = uuid::Uuid::new_v4().to_string();
    let msg = state
        .store
        .create_chat_message(&msg_id, chat_id, "user", content, None)
        .map_err(to_error_response)?;

    Ok(Json(serde_json::json!(msg)))
}

/// Lane descriptions for the status endpoint.
const LANE_DESCRIPTIONS: &[(&str, &str)] = &[
    ("comm", "Inter-agent messages"),
    ("desktop", "Desktop automation (one mouse/keyboard)"),
    ("dev", "Developer assistant (serialized)"),
    ("events", "Cron jobs and scheduled tasks"),
    ("heartbeat", "Proactive heartbeat ticks"),
    ("main", "User chat (web, CLI, voice)"),
    ("nested", "Tool recursion"),
    ("subagent", "Sub-agent tasks"),
];

/// GET /api/v1/agent/lanes
pub async fn get_lanes(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let status = state.lanes.status();
    let desc_map: std::collections::HashMap<&str, &str> = LANE_DESCRIPTIONS.iter().copied().collect();

    let lanes: Vec<serde_json::Value> = status
        .iter()
        .map(|(name, active, queued, max)| {
            let description = desc_map.get(name.as_str()).unwrap_or(&"");
            serde_json::json!({
                "name": name,
                "description": description,
                "concurrency": max,
                "active": active,
                "queued": queued,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({"lanes": lanes})))
}

/// GET /api/v1/agent/advisors
pub async fn list_advisors(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let advisors = state.store.list_advisors().map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"advisors": advisors})))
}

/// GET /api/v1/agent/advisors/:name
pub async fn get_advisor(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let advisor = state
        .store
        .get_advisor_by_name(&name)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    Ok(Json(serde_json::json!(advisor)))
}

/// POST /api/v1/agent/advisors
pub async fn create_advisor(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let name = body["name"]
        .as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("name required".into())))?;
    let role = body["role"].as_str().unwrap_or("advisor");
    let description = body["description"].as_str().unwrap_or("");
    let persona = body["persona"].as_str().unwrap_or("");
    let priority = body["priority"].as_i64().unwrap_or(50);
    let timeout = body["timeoutSeconds"].as_i64().unwrap_or(30);

    let advisor = state
        .store
        .create_advisor(name, role, description, priority, persona, timeout)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!(advisor)))
}

/// PUT /api/v1/agent/advisors/:name
pub async fn update_advisor(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let advisor = state
        .store
        .get_advisor_by_name(&name)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    state
        .store
        .update_advisor(
            advisor.id,
            body["role"].as_str(),
            body["description"].as_str(),
            body["priority"].as_i64(),
            body["persona"].as_str(),
            body["enabled"].as_i64().map(|v| v != 0),
            body["memoryAccess"].as_i64().map(|v| v != 0),
            body["timeoutSeconds"].as_i64(),
        )
        .map_err(to_error_response)?;

    let updated = state.store.get_advisor_by_name(&name).map_err(to_error_response)?;
    Ok(Json(serde_json::json!(updated)))
}

/// DELETE /api/v1/agent/advisors/:name
pub async fn delete_advisor(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let advisor = state
        .store
        .get_advisor_by_name(&name)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    state.store.delete_advisor(advisor.id).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"success": true})))
}

/// GET /api/v1/agent/heartbeat
pub async fn get_heartbeat(
    State(_state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let data_dir = config::data_dir().map_err(to_error_response)?;
    let path = data_dir.join("HEARTBEAT.md");
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    Ok(Json(serde_json::json!({"content": content})))
}

/// PUT /api/v1/agent/heartbeat
pub async fn update_heartbeat(
    State(_state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let content = body["content"].as_str().unwrap_or("");
    let data_dir = config::data_dir().map_err(to_error_response)?;
    // Best-effort: data dir should already exist from startup
    let _ = std::fs::create_dir_all(&data_dir);
    std::fs::write(data_dir.join("HEARTBEAT.md"), content)
        .map_err(|e| to_error_response(types::NeboError::Internal(e.to_string())))?;
    Ok(Json(serde_json::json!({"success": true})))
}

/// GET /api/v1/update/check
pub async fn update_check() -> HandlerResult<serde_json::Value> {
    let result = updater::check(crate::VERSION)
        .await
        .map_err(|e| to_error_response(types::NeboError::Internal(e.to_string())))?;
    Ok(Json(serde_json::json!(result)))
}

/// POST /api/v1/update/apply
pub async fn update_apply(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    // Try to use a staged binary first (already downloaded + verified)
    let pending = state.update_pending.lock().await.take();

    let binary_path = if let Some((path, _version)) = pending {
        path
    } else {
        // No staged binary — download fresh
        let result = updater::check(crate::VERSION)
            .await
            .map_err(|e| to_error_response(types::NeboError::Internal(e.to_string())))?;
        if !result.available {
            return Ok(Json(serde_json::json!({"status": "no_update"})));
        }
        let path = updater::download(&result.latest_version, None)
            .await
            .map_err(|e| to_error_response(types::NeboError::Internal(e.to_string())))?;
        updater::verify_checksum(&path, &result.latest_version)
            .await
            .map_err(|e| to_error_response(types::NeboError::Internal(e.to_string())))?;
        path
    };

    // Respond first, then apply — so the client sees the success response
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if let Err(e) = updater::apply_update(&binary_path) {
            tracing::error!("update apply failed: {}", e);
        }
    });

    Ok(Json(serde_json::json!({"status": "restarting"})))
}
