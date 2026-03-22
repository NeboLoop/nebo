//! Heartbeat scheduler — fires prompt-based proactive tasks for entities
//! with heartbeat enabled (main agent, roles, channels).
//!
//! Coexists with RoleWorker workflow-bound heartbeats: RoleWorker runs
//! workflows, this scheduler runs prompt-based chat dispatches.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use tools::Origin;
use types::constants::lanes;

use crate::chat_dispatch::{ChatConfig, run_chat};
use crate::entity_config;
use crate::state::AppState;

/// In-memory tracker for last-fire times per entity.
type LastFired = Arc<Mutex<HashMap<String, Instant>>>;

/// Spawn the heartbeat scheduler. Polls every 60 seconds.
pub fn spawn(state: AppState) {
    let last_fired: LastFired = Arc::new(Mutex::new(HashMap::new()));

    tokio::spawn(async move {
        // Initial delay to let the server boot
        tokio::time::sleep(Duration::from_secs(15)).await;

        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            if let Err(e) = tick(&state, &last_fired).await {
                warn!("heartbeat tick error: {}", e);
            }
        }
    });
}

async fn tick(state: &AppState, last_fired: &LastFired) -> Result<(), String> {
    // Load global settings for resolution
    let settings = state
        .store
        .get_settings()
        .map_err(|e| e.to_string())?
        .unwrap_or_else(|| db::models::Setting {
            id: 1,
            autonomous_mode: 0,
            auto_approve_read: 0,
            auto_approve_write: 0,
            auto_approve_bash: 0,
            heartbeat_interval_minutes: 0,
            comm_enabled: 0,
            comm_plugin: String::new(),
            developer_mode: 0,
            auto_update: 1,
            updated_at: 0,
        });

    let global_permissions: HashMap<String, bool> = state
        .store
        .get_user_profile()
        .ok()
        .flatten()
        .and_then(|p| p.tool_permissions)
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();

    let heartbeat_md = config::data_dir()
        .ok()
        .map(|d| std::fs::read_to_string(d.join("HEARTBEAT.md")).unwrap_or_default())
        .unwrap_or_default();

    // Collect entities to check: explicitly enabled + main entity if global interval > 0
    let mut entities = state
        .store
        .list_heartbeat_entities()
        .map_err(|e| e.to_string())?;

    // Also check the main entity: if global heartbeat is set and no explicit entity_config
    // override disables it, fire for main.
    let main_config = state
        .store
        .get_entity_config("main", "main")
        .map_err(|e| e.to_string())?;

    let main_explicitly_listed = entities.iter().any(|e| e.entity_type == "main" && e.entity_id == "main");
    if !main_explicitly_listed && settings.heartbeat_interval_minutes > 0 {
        // Main entity uses global settings — check if not explicitly disabled
        let disabled = main_config
            .as_ref()
            .and_then(|c| c.heartbeat_enabled)
            .map(|v| v == 0)
            .unwrap_or(false);
        if !disabled {
            if let Some(mc) = main_config.clone() {
                entities.push(mc);
            } else {
                // No entity_config row yet — use synthetic defaults
                entities.push(db::models::EntityConfig {
                    id: 0,
                    entity_type: "main".into(),
                    entity_id: "main".into(),
                    heartbeat_enabled: None,
                    heartbeat_interval_minutes: None,
                    heartbeat_content: None,
                    heartbeat_window_start: None,
                    heartbeat_window_end: None,
                    permissions: None,
                    resource_grants: None,
                    model_preference: None,
                    personality_snippet: None,
                    allowed_paths: None,
                    created_at: 0,
                    updated_at: 0,
                });
            }
        }
    }

    let now = Instant::now();
    let mut fired = last_fired.lock().await;

    for entity in &entities {
        let resolved = entity_config::resolve(
            &entity.entity_type,
            &entity.entity_id,
            Some(entity),
            &settings,
            &global_permissions,
            &heartbeat_md,
        );

        if !resolved.heartbeat_enabled || resolved.heartbeat_interval_minutes <= 0 {
            continue;
        }

        let key = format!("{}-{}", entity.entity_type, entity.entity_id);
        let interval_dur = Duration::from_secs(resolved.heartbeat_interval_minutes as u64 * 60);

        // Check if enough time has elapsed
        if let Some(last) = fired.get(&key) {
            if now.duration_since(*last) < interval_dur {
                continue;
            }
        }

        // Check time window
        if let Some((start, end)) = &resolved.heartbeat_window {
            if !in_time_window(start, end) {
                debug!(entity = key, "heartbeat outside time window, skipping");
                continue;
            }
        }

        // Skip if heartbeat content is empty
        if resolved.heartbeat_content.trim().is_empty() {
            continue;
        }

        // Skip deactivated roles — check the live registry
        if entity.entity_type == "role" {
            let registry = state.role_registry.read().await;
            if !registry.contains_key(&entity.entity_id) {
                continue;
            }
        }

        info!(entity = key, "firing heartbeat");

        let session_key = format!("heartbeat-{}-{}", entity.entity_type, entity.entity_id);
        let role_id = if entity.entity_type == "role" {
            entity.entity_id.clone()
        } else {
            String::new()
        };

        let config = ChatConfig {
            session_key,
            prompt: resolved.heartbeat_content.clone(),
            system: String::new(),
            user_id: String::new(),
            channel: "heartbeat".into(),
            origin: Origin::System,
            role_id,
            cancel_token: CancellationToken::new(),
            lane: lanes::HEARTBEAT.to_string(),
            comm_reply: None,
            entity_config: Some(resolved.clone()),
            images: vec![],
        };

        run_chat(state, config, None).await;
        fired.insert(key, now);
    }

    Ok(())
}

/// Check if the current local time is within the given HH:MM window.
fn in_time_window(start: &str, end: &str) -> bool {
    let now = chrono::Local::now().format("%H:%M").to_string();
    let now = now.as_str();
    if start <= end {
        now >= start && now <= end
    } else {
        // Window wraps midnight (e.g., 22:00 - 06:00)
        now >= start || now <= end
    }
}
