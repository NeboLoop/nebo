//! Heartbeats — prompt-based proactive turns for entities with heartbeat
//! enabled (main agent, agents, channels). WHEN a heartbeat fires is the
//! engine's: one pending timer per enabled entity, re-armed from the last
//! consumed one (`crate::engine`). This module says WHICH entities are due
//! for a timer and HOW one fires.
//!
//! Coexists with AgentWorker workflow-bound heartbeats: AgentWorker runs
//! workflows, this runs prompt-based chat dispatches.

use std::collections::HashMap;

use chrono::{Datelike, Local, NaiveTime, TimeZone};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use tools::Origin;
use types::constants::lanes;

use crate::chat_dispatch::{ChatConfig, run_chat};
use crate::entity_config::{self, ResolvedEntityConfig};
use crate::state::AppState;

/// One entity the engine should hold a timer for.
pub(crate) struct Enabled {
    pub entity_type: String,
    pub entity_id: String,
    pub interval_secs: i64,
    pub window: Option<(String, String)>,
    /// The entity's last fire before the engine held its timers, if any —
    /// the floor for the first arming.
    pub last_fired_at: Option<i64>,
}

impl Enabled {
    /// The timer target: `heartbeat:<type>:<id>`.
    pub fn target(&self) -> String {
        format!("heartbeat:{}:{}", self.entity_type, self.entity_id)
    }
}

/// Every entity whose heartbeat is on, resolved against global settings:
/// explicitly enabled rows, plus main when the global interval is set and
/// main is not explicitly off. Entities with nothing to say, and agents no
/// longer in the live registry, are not due for a timer.
pub(crate) async fn enabled_entities(state: &AppState) -> Result<Vec<Enabled>, String> {
    let (settings, global_permissions, heartbeat_md) = context(state)?;

    let mut entities = state
        .store
        .list_heartbeat_entities()
        .map_err(|e| e.to_string())?;

    let main_config = state
        .store
        .get_entity_config("main", "main")
        .map_err(|e| e.to_string())?;
    let main_explicitly_listed = entities
        .iter()
        .any(|e| e.entity_type == "main" && e.entity_id == "main");
    if !main_explicitly_listed && settings.heartbeat_interval_minutes > 0 {
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
                    pinned: None,
                    multi_chat: None,
                    operation_policy: None,
                    learning_mode: None,
                    created_at: 0,
                    updated_at: 0,
                    last_heartbeat_at: None,
                });
            }
        }
    }

    let mut out = Vec::new();
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
        if resolved.heartbeat_content.trim().is_empty() {
            continue;
        }
        // Skip deactivated agents — check the live registry
        if entity.entity_type == "agent" {
            let registry = state.agent_registry.read().await;
            if !registry.contains_key(&entity.entity_id) {
                continue;
            }
        }
        out.push(Enabled {
            entity_type: entity.entity_type.clone(),
            entity_id: entity.entity_id.clone(),
            interval_secs: resolved.heartbeat_interval_minutes * 60,
            window: resolved.heartbeat_window.clone(),
            last_fired_at: entity.last_heartbeat_at.as_deref().and_then(|s| s.parse().ok()),
        });
    }
    Ok(out)
}

/// Fire one heartbeat: resolve the entity fresh (content and window may
/// have changed since the timer was armed) and run the chat on the
/// heartbeat lane. Ok(false) means it was not fired — disabled or empty by
/// the time it came due.
pub(crate) async fn fire(state: &AppState, entity_type: &str, entity_id: &str) -> Result<bool, String> {
    let (settings, global_permissions, heartbeat_md) = context(state)?;
    let entity = state
        .store
        .get_entity_config(entity_type, entity_id)
        .map_err(|e| e.to_string())?;
    let resolved: ResolvedEntityConfig = entity_config::resolve(
        entity_type,
        entity_id,
        entity.as_ref(),
        &settings,
        &global_permissions,
        &heartbeat_md,
    );
    if !resolved.heartbeat_enabled || resolved.heartbeat_content.trim().is_empty() {
        return Ok(false);
    }

    let key = format!("{entity_type}-{entity_id}");
    info!(entity = key, "firing heartbeat");

    let config = ChatConfig {
        session_key: format!("heartbeat-{entity_type}-{entity_id}"),
        prompt: resolved.heartbeat_content.clone(),
        system: String::new(),
        user_id: String::new(),
        channel: "heartbeat".into(),
        origin: Origin::System,
        agent_id: if entity_type == "agent" { entity_id.to_string() } else { String::new() },
        cancel_token: CancellationToken::new(),
        lane: lanes::HEARTBEAT.to_string(),
        comm_reply: None,
        entity_config: Some(resolved.clone()),
        images: vec![],
        entity_name: String::new(),
        origin_agent_id: None,
        mention_context: None,
        tool_scope: None,
        plan_mode: false,
        channel_ctx: None,
        handoff_depth: 0,
        seed_taint: vec![],
        tool_allowlist: None,
        hidden_prompt: false,
        audience: None,
        cwd: None,
        model_override: None,
    };

    run_chat(state, config).await;

    // The entity row keeps its last-fired stamp for the settings UI.
    let epoch = chrono::Utc::now().timestamp().to_string();
    if let Err(e) = state.store.update_heartbeat_at(entity_type, entity_id, &epoch) {
        warn!(entity = %key, error = %e, "failed to persist heartbeat timestamp");
    }
    Ok(true)
}

fn context(state: &AppState) -> Result<(db::models::Setting, HashMap<String, bool>, String), String> {
    let settings = state
        .store
        .get_settings()
        .map_err(|e| e.to_string())?
        .unwrap_or_else(|| db::models::Setting {
            id: 1,
            auto_install_deps: 0,
            auto_approve_read: 0,
            auto_approve_write: 0,
            auto_approve_bash: 0,
            heartbeat_interval_minutes: 0,
            comm_enabled: 0,
            comm_plugin: String::new(),
            developer_mode: 0,
            auto_update: 1,
            full_access: 0,
            guardrails: serde_json::json!({}),
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
    Ok((settings, global_permissions, heartbeat_md))
}

/// The first moment at or after `due` that falls inside the HH:MM window
/// on the local clock. A window that wraps midnight (22:00–06:00) is
/// honored. An unparseable window is no window.
pub(crate) fn next_in_window(due: i64, window: Option<&(String, String)>) -> i64 {
    let Some((start, end)) = window else { return due };
    let (Ok(start), Ok(end)) = (NaiveTime::parse_from_str(start, "%H:%M"), NaiveTime::parse_from_str(end, "%H:%M")) else {
        return due;
    };
    let Some(at) = Local.timestamp_opt(due, 0).single() else { return due };
    let t = at.time();
    let inside = if start <= end { t >= start && t <= end } else { t >= start || t <= end };
    if inside {
        return due;
    }
    // Next opening: today's start if still ahead, else tomorrow's.
    let today = at.date_naive();
    let candidate = if t < start { today } else { today + chrono::Days::new(1) };
    let _ = candidate.day(); // a date, not a duration: DST-safe
    Local
        .from_local_datetime(&candidate.and_time(start))
        .single()
        .map(|d| d.timestamp())
        .unwrap_or(due)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(h: u32, m: u32) -> i64 {
        Local.with_ymd_and_hms(2026, 8, 23, h, m, 0).single().unwrap().timestamp()
    }

    #[test]
    fn a_due_moment_outside_the_window_moves_to_the_next_opening() {
        let w = Some(("09:00".to_string(), "17:00".to_string()));
        assert_eq!(next_in_window(local(10, 30), w.as_ref()), local(10, 30), "inside stays");
        assert_eq!(next_in_window(local(6, 0), w.as_ref()), local(9, 0), "before opening → today's opening");
        let tomorrow = Local.with_ymd_and_hms(2026, 8, 24, 9, 0, 0).single().unwrap().timestamp();
        assert_eq!(next_in_window(local(18, 0), w.as_ref()), tomorrow, "after closing → tomorrow's opening");
        // Wrapping window 22:00–06:00: 23:00 is inside, 12:00 waits for 22:00.
        let night = Some(("22:00".to_string(), "06:00".to_string()));
        assert_eq!(next_in_window(local(23, 0), night.as_ref()), local(23, 0));
        assert_eq!(next_in_window(local(12, 0), night.as_ref()), local(22, 0));
        assert_eq!(next_in_window(local(12, 0), None), local(12, 0));
        assert_eq!(next_in_window(local(12, 0), Some(&("x".to_string(), "y".to_string()))), local(12, 0));
    }
}
