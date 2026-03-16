//! RoleWorker — autonomous role execution.
//!
//! One `RoleWorker` per active role. Spawns and owns all trigger tasks
//! (schedule, heartbeat, event). The `RoleWorkerRegistry` manages the
//! lifecycle of all workers.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use db::Store;
use tools::workflows::WorkflowManager;
use workflow::events::{EventDispatcher, EventSubscription};

/// A single autonomous role worker. Owns all trigger tasks for one role.
pub struct RoleWorker {
    pub role_id: String,
    pub name: String,
    cancel: CancellationToken,
    event_dispatcher: Arc<EventDispatcher>,
    workflow_manager: Arc<dyn WorkflowManager>,
}

impl RoleWorker {
    /// Start the worker: read bindings from DB, resolve role config, spawn trigger tasks.
    pub fn start(
        role_id: String,
        name: String,
        store: &Store,
        workflow_manager: Arc<dyn WorkflowManager>,
        event_dispatcher: Arc<EventDispatcher>,
    ) -> Self {
        let cancel = CancellationToken::new();

        let bindings = match store.list_role_workflows(&role_id) {
            Ok(b) => b,
            Err(e) => {
                warn!(role = %role_id, error = %e, "failed to load role workflow bindings");
                return Self { role_id, name, cancel, event_dispatcher, workflow_manager };
            }
        };

        // Load role config from DB frontmatter to get inline activities
        let role_config = match store.get_role(&role_id) {
            Ok(Some(r)) => match napp::role::parse_role_config(&r.frontmatter) {
                Ok(cfg) => Some(cfg),
                Err(e) => {
                    warn!(role = %role_id, error = %e, "failed to parse role config frontmatter");
                    None
                }
            },
            Ok(None) => {
                warn!(role = %role_id, "role not found in DB");
                None
            }
            Err(e) => {
                warn!(role = %role_id, error = %e, "failed to load role from DB");
                None
            }
        };

        // Schedule triggers: delegate to existing cron system
        workflow::triggers::register_role_triggers(&role_id, &bindings, store);

        for binding in &bindings {
            // Look up the WorkflowBinding from role config to get activities
            let wf_binding = role_config
                .as_ref()
                .and_then(|c| c.workflows.get(&binding.binding_name));

            match binding.trigger_type.as_str() {
                "heartbeat" => {
                    let (duration, window) = parse_heartbeat(&binding.trigger_config);
                    if duration.is_zero() {
                        warn!(
                            role = %role_id,
                            binding = %binding.binding_name,
                            config = %binding.trigger_config,
                            "invalid heartbeat config, skipping"
                        );
                        continue;
                    }
                    // Build inline definition JSON from role config
                    let def_json = match wf_binding {
                        Some(wb) if wb.has_activities() => wb.to_workflow_json(&binding.binding_name),
                        _ => {
                            warn!(role = %role_id, binding = %binding.binding_name, "no inline activities found, skipping heartbeat");
                            continue;
                        }
                    };
                    let inputs: serde_json::Value = binding
                        .inputs
                        .as_ref()
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or_default();
                    // Build emit_source from binding: "{role-slug}.{emit-name}"
                    let emit_source = wf_binding
                        .and_then(|wb| wb.emit.as_ref())
                        .map(|emit_name| {
                            let slug = name.to_lowercase().replace(' ', "-");
                            format!("{}.{}", slug, emit_name)
                        });
                    let mgr = workflow_manager.clone();
                    let role = role_id.clone();
                    let bname = binding.binding_name.clone();
                    let token = cancel.clone();

                    tokio::spawn(async move {
                        let mut interval = tokio::time::interval(duration);
                        interval.tick().await; // skip first immediate tick

                        loop {
                            tokio::select! {
                                _ = interval.tick() => {
                                    // Check time window if configured
                                    if let Some((start, end)) = &window {
                                        let now = chrono::Local::now().time();
                                        if now < *start || now > *end {
                                            continue;
                                        }
                                    }

                                    match mgr.run_inline(def_json.clone(), inputs.clone(), "heartbeat", &role, emit_source.clone()).await {
                                        Ok(run_id) => {
                                            info!(
                                                role = %role,
                                                binding = %bname,
                                                run_id = %run_id,
                                                "heartbeat triggered inline workflow"
                                            );
                                            notify_crate::send("Nebo", &format!("Heartbeat: {}", bname));
                                        }
                                        Err(e) => {
                                            warn!(
                                                role = %role,
                                                binding = %bname,
                                                error = %e,
                                                "heartbeat inline workflow run failed"
                                            );
                                            notify_crate::send("Nebo", &format!("{} failed: {}", bname, e));
                                        }
                                    }
                                }
                                _ = token.cancelled() => break,
                            }
                        }
                    });

                    info!(
                        role = %role_id,
                        binding = %binding.binding_name,
                        interval = ?duration,
                        window = ?window,
                        "started heartbeat trigger"
                    );
                }
                "event" => {
                    // Build inline definition JSON from role config
                    let def_json = wf_binding
                        .filter(|wb| wb.has_activities())
                        .map(|wb| wb.to_workflow_json(&binding.binding_name));

                    let inputs: serde_json::Value = binding
                        .inputs
                        .as_ref()
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or_default();

                    // Build emit_source from binding: "{role-slug}.{emit-name}"
                    let event_emit_source = wf_binding
                        .and_then(|wb| wb.emit.as_ref())
                        .map(|emit_name| {
                            let slug = name.to_lowercase().replace(' ', "-");
                            format!("{}.{}", slug, emit_name)
                        });

                    for source in binding.trigger_config.split(',') {
                        let pattern = source.trim().to_string();
                        if pattern.is_empty() {
                            continue;
                        }
                        let dispatcher = event_dispatcher.clone();
                        let sub = EventSubscription {
                            pattern,
                            default_inputs: inputs.clone(),
                            role_source: role_id.clone(),
                            binding_name: binding.binding_name.clone(),
                            definition_json: def_json.clone(),
                            emit_source: event_emit_source.clone(),
                        };
                        tokio::spawn(async move {
                            dispatcher.subscribe(sub).await;
                        });
                    }
                }
                "schedule" => {
                    // Already handled by register_role_triggers above
                }
                "manual" => {
                    // No-op: user triggers via chat
                }
                other => {
                    warn!(
                        role = %role_id,
                        binding = %binding.binding_name,
                        trigger_type = %other,
                        "unknown trigger type"
                    );
                }
            }
        }

        info!(role = %role_id, name = %name, bindings = bindings.len(), "role worker started");

        Self { role_id, name, cancel, event_dispatcher, workflow_manager }
    }

    /// Stop the worker: cancel all spawned tasks, running workflows, cron jobs and event subscriptions.
    pub fn stop(&self, store: &Store) {
        self.cancel.cancel();
        workflow::triggers::unregister_role_triggers(&self.role_id, store);
        // Cancel any running workflows spawned by this role
        let mgr = self.workflow_manager.clone();
        let role_id_wf = self.role_id.clone();
        tokio::spawn(async move {
            mgr.cancel_runs_for_role(&role_id_wf).await;
        });
        // Clean up event subscriptions (async, fire-and-forget)
        let dispatcher = self.event_dispatcher.clone();
        let role_id = self.role_id.clone();
        tokio::spawn(async move {
            dispatcher.unsubscribe_role(&role_id).await;
        });
        info!(role = %self.role_id, "role worker stopped");
    }
}

/// Registry of all active role workers.
pub struct RoleWorkerRegistry {
    workers: RwLock<HashMap<String, RoleWorker>>,
    store: Arc<Store>,
    workflow_manager: Arc<dyn WorkflowManager>,
    event_dispatcher: Arc<EventDispatcher>,
}

impl RoleWorkerRegistry {
    pub fn new(
        store: Arc<Store>,
        workflow_manager: Arc<dyn WorkflowManager>,
        event_dispatcher: Arc<EventDispatcher>,
    ) -> Self {
        Self {
            workers: RwLock::new(HashMap::new()),
            store,
            workflow_manager,
            event_dispatcher,
        }
    }

    /// Start a role worker. If already running, stops the old one first.
    pub async fn start_role(&self, role_id: &str, name: &str) {
        // Stop existing worker if any
        {
            let mut workers = self.workers.write().await;
            if let Some(old) = workers.remove(role_id) {
                old.stop(&self.store);
            }
        }

        let worker = RoleWorker::start(
            role_id.to_string(),
            name.to_string(),
            &self.store,
            self.workflow_manager.clone(),
            self.event_dispatcher.clone(),
        );

        self.workers.write().await.insert(role_id.to_string(), worker);
    }

    /// Stop a role worker.
    pub async fn stop_role(&self, role_id: &str) {
        let mut workers = self.workers.write().await;
        if let Some(worker) = workers.remove(role_id) {
            worker.stop(&self.store);
        }
    }

    /// Stop all workers (shutdown).
    pub async fn stop_all(&self) {
        let mut workers = self.workers.write().await;
        for (_, worker) in workers.drain() {
            worker.stop(&self.store);
        }
    }
}

/// Parse a heartbeat trigger config string.
///
/// Format: `"30m"` or `"30m|08:00-18:00"`
///
/// Returns `(Duration, Option<(NaiveTime, NaiveTime)>)`.
fn parse_heartbeat(config: &str) -> (std::time::Duration, Option<(chrono::NaiveTime, chrono::NaiveTime)>) {
    let parts: Vec<&str> = config.split('|').collect();
    let duration = parse_duration(parts[0].trim());
    let window = if parts.len() > 1 {
        parse_time_window(parts[1].trim())
    } else {
        None
    };
    (duration, window)
}

/// Parse a duration string like "30m", "1h", "5s", "2h30m".
fn parse_duration(s: &str) -> std::time::Duration {
    let s = s.trim();
    let mut total_secs: u64 = 0;
    let mut num_buf = String::new();

    for c in s.chars() {
        if c.is_ascii_digit() {
            num_buf.push(c);
        } else {
            let n: u64 = num_buf.parse().unwrap_or(0);
            num_buf.clear();
            match c {
                'h' => total_secs += n * 3600,
                'm' => total_secs += n * 60,
                's' => total_secs += n,
                _ => {}
            }
        }
    }

    // If there's a trailing number with no unit, treat as minutes
    if !num_buf.is_empty() {
        let n: u64 = num_buf.parse().unwrap_or(0);
        total_secs += n * 60;
    }

    std::time::Duration::from_secs(total_secs)
}

/// Parse a time window like "08:00-18:00".
fn parse_time_window(s: &str) -> Option<(chrono::NaiveTime, chrono::NaiveTime)> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 2 {
        return None;
    }
    let start = chrono::NaiveTime::parse_from_str(parts[0].trim(), "%H:%M").ok()?;
    let end = chrono::NaiveTime::parse_from_str(parts[1].trim(), "%H:%M").ok()?;
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_minutes() {
        assert_eq!(parse_duration("30m"), std::time::Duration::from_secs(1800));
    }

    #[test]
    fn test_parse_duration_hours() {
        assert_eq!(parse_duration("1h"), std::time::Duration::from_secs(3600));
    }

    #[test]
    fn test_parse_duration_combined() {
        assert_eq!(parse_duration("1h30m"), std::time::Duration::from_secs(5400));
    }

    #[test]
    fn test_parse_duration_seconds() {
        assert_eq!(parse_duration("90s"), std::time::Duration::from_secs(90));
    }

    #[test]
    fn test_parse_duration_bare_number() {
        assert_eq!(parse_duration("5"), std::time::Duration::from_secs(300));
    }

    #[test]
    fn test_parse_heartbeat_simple() {
        let (d, w) = parse_heartbeat("30m");
        assert_eq!(d, std::time::Duration::from_secs(1800));
        assert!(w.is_none());
    }

    #[test]
    fn test_parse_heartbeat_with_window() {
        let (d, w) = parse_heartbeat("30m|08:00-18:00");
        assert_eq!(d, std::time::Duration::from_secs(1800));
        let (start, end) = w.unwrap();
        assert_eq!(start, chrono::NaiveTime::from_hms_opt(8, 0, 0).unwrap());
        assert_eq!(end, chrono::NaiveTime::from_hms_opt(18, 0, 0).unwrap());
    }

    #[test]
    fn test_parse_time_window() {
        let w = parse_time_window("09:30-17:00").unwrap();
        assert_eq!(w.0, chrono::NaiveTime::from_hms_opt(9, 30, 0).unwrap());
        assert_eq!(w.1, chrono::NaiveTime::from_hms_opt(17, 0, 0).unwrap());
    }

    #[test]
    fn test_parse_time_window_invalid() {
        assert!(parse_time_window("invalid").is_none());
    }
}
