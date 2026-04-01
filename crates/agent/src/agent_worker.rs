//! AgentWorker — autonomous agent execution.
//!
//! One `AgentWorker` per active agent. Spawns and owns all trigger tasks
//! (schedule, heartbeat, event, watch). The `AgentWorkerRegistry` manages the
//! lifecycle of all workers.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use db::Store;
use napp::plugin::PluginStore;
use tools::workflows::WorkflowManager;
use workflow::events::{EventDispatcher, EventSubscription};

/// A single autonomous agent worker. Owns all trigger tasks for one agent.
pub struct AgentWorker {
    pub agent_id: String,
    pub name: String,
    cancel: CancellationToken,
    event_dispatcher: Arc<EventDispatcher>,
    workflow_manager: Arc<dyn WorkflowManager>,
    plugin_store: Arc<PluginStore>,
}

impl AgentWorker {
    /// Start the worker: read bindings from DB, resolve agent config, spawn trigger tasks.
    pub fn start(
        agent_id: String,
        name: String,
        store: &Store,
        workflow_manager: Arc<dyn WorkflowManager>,
        event_dispatcher: Arc<EventDispatcher>,
        plugin_store: Arc<PluginStore>,
    ) -> Self {
        let cancel = CancellationToken::new();

        let bindings = match store.list_agent_workflows(&agent_id) {
            Ok(b) => b,
            Err(e) => {
                warn!(agent = %agent_id, error = %e, "failed to load agent workflow bindings");
                return Self { agent_id, name, cancel, event_dispatcher, workflow_manager, plugin_store };
            }
        };

        // Load agent config from DB frontmatter to get inline activities
        let agent_config = match store.get_agent(&agent_id) {
            Ok(Some(r)) => match napp::agent::parse_agent_config(&r.frontmatter) {
                Ok(cfg) => Some(cfg),
                Err(e) => {
                    warn!(agent = %agent_id, error = %e, "failed to parse agent config frontmatter");
                    None
                }
            },
            Ok(None) => {
                warn!(agent = %agent_id, "agent not found in DB");
                None
            }
            Err(e) => {
                warn!(agent = %agent_id, error = %e, "failed to load agent from DB");
                None
            }
        };

        // Schedule triggers: delegate to existing cron system
        workflow::triggers::register_agent_triggers(&agent_id, &bindings, store);

        for binding in &bindings {
            // Look up the WorkflowBinding from agent config to get activities
            let wf_binding = agent_config
                .as_ref()
                .and_then(|c| c.workflows.get(&binding.binding_name));

            match binding.trigger_type.as_str() {
                "heartbeat" => {
                    let (duration, window) = parse_heartbeat(&binding.trigger_config);
                    if duration.is_zero() {
                        warn!(
                            agent = %agent_id,
                            binding = %binding.binding_name,
                            config = %binding.trigger_config,
                            "invalid heartbeat config, skipping"
                        );
                        continue;
                    }
                    // Build inline definition JSON from agent config
                    let def_json = match wf_binding {
                        Some(wb) if wb.has_activities() => wb.to_workflow_json(&binding.binding_name),
                        _ => {
                            warn!(agent = %agent_id, binding = %binding.binding_name, "no inline activities found, skipping heartbeat");
                            continue;
                        }
                    };
                    let inputs: serde_json::Value = binding
                        .inputs
                        .as_ref()
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or_default();
                    // Build emit_source from binding: "{agent-slug}.{emit-name}"
                    let emit_source = wf_binding
                        .and_then(|wb| wb.emit.as_ref())
                        .map(|emit_name| {
                            let slug = name.to_lowercase().replace(' ', "-");
                            format!("{}.{}", slug, emit_name)
                        });
                    let mgr = workflow_manager.clone();
                    let agent = agent_id.clone();
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

                                    match mgr.run_inline(def_json.clone(), inputs.clone(), "heartbeat", &agent, emit_source.clone()).await {
                                        Ok(run_id) => {
                                            info!(
                                                agent = %agent,
                                                binding = %bname,
                                                run_id = %run_id,
                                                "heartbeat triggered inline workflow"
                                            );
                                            notify_crate::send("Nebo", &format!("Heartbeat: {}", bname));
                                        }
                                        Err(e) => {
                                            warn!(
                                                agent = %agent,
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
                        agent = %agent_id,
                        binding = %binding.binding_name,
                        interval = ?duration,
                        window = ?window,
                        "started heartbeat trigger"
                    );
                }
                "event" => {
                    // Build inline definition JSON from agent config
                    let def_json = wf_binding
                        .filter(|wb| wb.has_activities())
                        .map(|wb| wb.to_workflow_json(&binding.binding_name));

                    let inputs: serde_json::Value = binding
                        .inputs
                        .as_ref()
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or_default();

                    // Build emit_source from binding: "{agent-slug}.{emit-name}"
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
                            agent_source: agent_id.clone(),
                            binding_name: binding.binding_name.clone(),
                            definition_json: def_json.clone(),
                            emit_source: event_emit_source.clone(),
                        };
                        tokio::spawn(async move {
                            dispatcher.subscribe(sub).await;
                        });
                    }
                }
                "watch" => {
                    // Parse watch config from trigger_config JSON
                    let watch_cfg: WatchTriggerConfig = match serde_json::from_str(&binding.trigger_config) {
                        Ok(c) => c,
                        Err(e) => {
                            warn!(
                                agent = %agent_id,
                                binding = %binding.binding_name,
                                error = %e,
                                "invalid watch trigger config, skipping"
                            );
                            continue;
                        }
                    };

                    // Resolve plugin binary
                    let binary_path = match plugin_store.resolve(&watch_cfg.plugin, "*") {
                        Some(p) => p,
                        None => {
                            warn!(
                                agent = %agent_id,
                                binding = %binding.binding_name,
                                plugin = %watch_cfg.plugin,
                                "watch plugin not found, skipping"
                            );
                            continue;
                        }
                    };

                    // Build inline definition JSON from agent config
                    let def_json = match wf_binding {
                        Some(wb) if wb.has_activities() => wb.to_workflow_json(&binding.binding_name),
                        _ => {
                            warn!(agent = %agent_id, binding = %binding.binding_name, "no inline activities found, skipping watch");
                            continue;
                        }
                    };

                    let inputs: serde_json::Value = binding
                        .inputs
                        .as_ref()
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or_default();

                    // Substitute agent input values into the command template
                    let input_values: serde_json::Value = match store.get_agent(&agent_id) {
                        Ok(Some(r)) => serde_json::from_str(&r.input_values).unwrap_or_default(),
                        _ => serde_json::Value::default(),
                    };
                    let command = substitute_inputs(&watch_cfg.command, &input_values);

                    let emit_source = wf_binding
                        .and_then(|wb| wb.emit.as_ref())
                        .map(|emit_name| {
                            let slug = name.to_lowercase().replace(' ', "-");
                            format!("{}.{}", slug, emit_name)
                        });

                    let token = cancel.clone();
                    let mgr = workflow_manager.clone();
                    let agent = agent_id.clone();
                    let bname = binding.binding_name.clone();
                    let ps = plugin_store.clone();
                    let watch_plugin = watch_cfg.plugin.clone();

                    tokio::spawn(watch_loop(
                        binary_path,
                        command,
                        watch_cfg,
                        ps,
                        def_json,
                        inputs,
                        agent,
                        bname,
                        emit_source,
                        mgr,
                        token,
                    ));

                    info!(
                        agent = %agent_id,
                        binding = %binding.binding_name,
                        plugin = %watch_plugin,
                        "started watch trigger"
                    );
                }
                "schedule" => {
                    // Already handled by register_agent_triggers above
                }
                "manual" => {
                    // No-op: user triggers via chat
                }
                other => {
                    warn!(
                        agent = %agent_id,
                        binding = %binding.binding_name,
                        trigger_type = %other,
                        "unknown trigger type"
                    );
                }
            }
        }

        info!(agent = %agent_id, name = %name, bindings = bindings.len(), "agent worker started");

        Self { agent_id, name, cancel, event_dispatcher, workflow_manager, plugin_store }
    }

    /// Stop the worker: cancel all spawned tasks, running workflows, cron jobs and event subscriptions.
    pub fn stop(&self, store: &Store) {
        self.cancel.cancel();
        workflow::triggers::unregister_agent_triggers(&self.agent_id, store);
        // Cancel any running workflows spawned by this agent
        let mgr = self.workflow_manager.clone();
        let agent_id_wf = self.agent_id.clone();
        tokio::spawn(async move {
            mgr.cancel_runs_for_agent(&agent_id_wf).await;
        });
        // Clean up event subscriptions (async, fire-and-forget)
        let dispatcher = self.event_dispatcher.clone();
        let agent_id = self.agent_id.clone();
        tokio::spawn(async move {
            dispatcher.unsubscribe_agent(&agent_id).await;
        });
        info!(agent = %self.agent_id, "agent worker stopped");
    }
}

/// Registry of all active agent workers.
pub struct AgentWorkerRegistry {
    workers: RwLock<HashMap<String, AgentWorker>>,
    store: Arc<Store>,
    workflow_manager: Arc<dyn WorkflowManager>,
    event_dispatcher: Arc<EventDispatcher>,
    plugin_store: Arc<PluginStore>,
}

impl AgentWorkerRegistry {
    pub fn new(
        store: Arc<Store>,
        workflow_manager: Arc<dyn WorkflowManager>,
        event_dispatcher: Arc<EventDispatcher>,
        plugin_store: Arc<PluginStore>,
    ) -> Self {
        Self {
            workers: RwLock::new(HashMap::new()),
            store,
            workflow_manager,
            event_dispatcher,
            plugin_store,
        }
    }

    /// Start an agent worker. If already running, stops the old one first.
    pub async fn start_agent(&self, agent_id: &str, name: &str) {
        // Stop existing worker if any
        {
            let mut workers = self.workers.write().await;
            if let Some(old) = workers.remove(agent_id) {
                old.stop(&self.store);
            }
        }

        let worker = AgentWorker::start(
            agent_id.to_string(),
            name.to_string(),
            &self.store,
            self.workflow_manager.clone(),
            self.event_dispatcher.clone(),
            self.plugin_store.clone(),
        );

        self.workers.write().await.insert(agent_id.to_string(), worker);
    }

    /// Stop an agent worker.
    pub async fn stop_agent(&self, agent_id: &str) {
        let mut workers = self.workers.write().await;
        if let Some(worker) = workers.remove(agent_id) {
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

// ---------------------------------------------------------------------------
// Watch trigger
// ---------------------------------------------------------------------------

/// Deserialized watch trigger config from DB's trigger_config JSON.
#[derive(Debug, Clone, serde::Deserialize)]
struct WatchTriggerConfig {
    plugin: String,
    command: String,
    #[serde(default = "default_restart_delay")]
    restart_delay_secs: u64,
}

fn default_restart_delay() -> u64 {
    5
}

/// Substitute `{{key}}` placeholders in a template with values from agent inputs.
fn substitute_inputs(template: &str, inputs: &serde_json::Value) -> String {
    let mut result = template.to_string();
    if let Some(obj) = inputs.as_object() {
        for (key, val) in obj {
            if let Some(s) = val.as_str() {
                result = result.replace(&format!("{{{{{}}}}}", key), s);
            }
        }
    }
    result
}

/// Long-running loop that spawns a plugin watcher process, reads NDJSON lines
/// from stdout, and fires `run_inline()` for each line received.
///
/// On process exit (not cancelled): waits with exponential backoff, then restarts.
/// On cancel token: kills child process and breaks.
async fn watch_loop(
    binary_path: std::path::PathBuf,
    command: String,
    cfg: WatchTriggerConfig,
    plugin_store: Arc<PluginStore>,
    def_json: String,
    base_inputs: serde_json::Value,
    agent_id: String,
    binding_name: String,
    emit_source: Option<String>,
    workflow_manager: Arc<dyn WorkflowManager>,
    cancel: CancellationToken,
) {
    let mut backoff_secs = cfg.restart_delay_secs;
    let max_backoff_secs = 300; // 5 minutes

    loop {
        if cancel.is_cancelled() {
            break;
        }

        // Parse command string into args
        let args = match shlex::split(&command) {
            Some(a) => a,
            None => {
                error!(
                    agent = %agent_id,
                    binding = %binding_name,
                    command = %command,
                    "failed to parse watch command, stopping"
                );
                break;
            }
        };

        // Build the child process (same env setup as PluginTool::handle_exec)
        let mut cmd = tokio::process::Command::new(&binary_path);
        cmd.args(&args);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Clean env + inject sanitized env
        cmd.env_clear();
        for (k, v) in tools::process::sanitized_env() {
            cmd.env(k, v);
        }

        // Plugin binary env var (e.g., GWS_BIN=/path/to/gws)
        cmd.env(
            napp::plugin::plugin_env_var(&cfg.plugin),
            binary_path.to_string_lossy().as_ref(),
        );

        // Augmented PATH with all plugin directories
        cmd.env("PATH", plugin_store.path_with_plugins());

        // Auth env vars (client_id, client_secret, etc.)
        if let Some((_bin, auth)) = plugin_store.get_auth_info(&cfg.plugin) {
            for (k, v) in &auth.env {
                cmd.env(k, v);
            }
        }

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        info!(
            agent = %agent_id,
            binding = %binding_name,
            binary = %binary_path.display(),
            command = %command,
            "spawning watch process"
        );

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                error!(
                    agent = %agent_id,
                    binding = %binding_name,
                    error = %e,
                    "failed to spawn watch process, retrying in {}s",
                    backoff_secs
                );
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)) => {}
                    _ = cancel.cancelled() => break,
                }
                backoff_secs = (backoff_secs * 2).min(max_backoff_secs);
                continue;
            }
        };

        // Reset backoff on successful spawn
        backoff_secs = cfg.restart_delay_secs;

        let stdout = child.stdout.take().expect("stdout piped");
        let mut lines = BufReader::new(stdout).lines();

        // Spawn a task to log stderr
        let stderr = child.stderr.take();
        let stderr_agent = agent_id.clone();
        let stderr_binding = binding_name.clone();
        let stderr_handle = tokio::spawn(async move {
            if let Some(stderr) = stderr {
                let mut stderr_lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = stderr_lines.next_line().await {
                    debug!(
                        agent = %stderr_agent,
                        binding = %stderr_binding,
                        "watch stderr: {}",
                        line
                    );
                }
            }
        });

        // Read NDJSON lines until process exits or cancel
        loop {
            tokio::select! {
                line_result = lines.next_line() => {
                    match line_result {
                        Ok(Some(line)) => {
                            let line = line.trim().to_string();
                            if line.is_empty() {
                                continue;
                            }

                            // Parse JSON payload
                            let payload: serde_json::Value = match serde_json::from_str(&line) {
                                Ok(v) => v,
                                Err(e) => {
                                    warn!(
                                        agent = %agent_id,
                                        binding = %binding_name,
                                        error = %e,
                                        "invalid JSON from watch process, skipping line"
                                    );
                                    continue;
                                }
                            };

                            // Merge base inputs with watch payload
                            let mut run_inputs = base_inputs.clone();
                            if let Some(obj) = run_inputs.as_object_mut() {
                                obj.insert("_watch_payload".to_string(), payload);
                                obj.insert("_watch_source".to_string(), serde_json::Value::String(cfg.plugin.clone()));
                            }

                            match workflow_manager.run_inline(
                                def_json.clone(),
                                run_inputs,
                                "watch",
                                &agent_id,
                                emit_source.clone(),
                            ).await {
                                Ok(run_id) => {
                                    info!(
                                        agent = %agent_id,
                                        binding = %binding_name,
                                        run_id = %run_id,
                                        "watch triggered inline workflow"
                                    );
                                }
                                Err(e) => {
                                    warn!(
                                        agent = %agent_id,
                                        binding = %binding_name,
                                        error = %e,
                                        "watch inline workflow run failed"
                                    );
                                    notify_crate::send("Nebo", &format!("{} failed: {}", binding_name, e));
                                }
                            }
                        }
                        Ok(None) => {
                            // stdout closed — process exiting
                            info!(agent = %agent_id, binding = %binding_name, "watch process stdout closed");
                            break;
                        }
                        Err(e) => {
                            warn!(agent = %agent_id, binding = %binding_name, error = %e, "watch stdout read error");
                            break;
                        }
                    }
                }
                _ = cancel.cancelled() => {
                    info!(agent = %agent_id, binding = %binding_name, "watch cancelled, killing process");
                    let _ = child.kill().await;
                    stderr_handle.abort();
                    return;
                }
            }
        }

        // Wait for the child to finish
        let _ = child.wait().await;
        stderr_handle.abort();

        if cancel.is_cancelled() {
            break;
        }

        // Restart with backoff
        info!(
            agent = %agent_id,
            binding = %binding_name,
            backoff_secs = backoff_secs,
            "watch process exited, restarting"
        );
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)) => {}
            _ = cancel.cancelled() => break,
        }
        backoff_secs = (backoff_secs * 2).min(max_backoff_secs);
    }
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

    #[test]
    fn test_substitute_inputs() {
        let inputs = serde_json::json!({
            "gcp_project": "my-project-123",
            "poll_interval": "30"
        });
        let template = "gmail +watch --project {{gcp_project}} --poll-interval {{poll_interval}}";
        let result = substitute_inputs(template, &inputs);
        assert_eq!(result, "gmail +watch --project my-project-123 --poll-interval 30");
    }

    #[test]
    fn test_substitute_inputs_no_match() {
        let inputs = serde_json::json!({});
        let template = "gmail +watch --project {{gcp_project}}";
        let result = substitute_inputs(template, &inputs);
        assert_eq!(result, template); // unchanged
    }

    #[test]
    fn test_watch_trigger_config_deserialize() {
        let json = r#"{"plugin":"gws","command":"gmail +watch","restart_delay_secs":10}"#;
        let cfg: WatchTriggerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.plugin, "gws");
        assert_eq!(cfg.command, "gmail +watch");
        assert_eq!(cfg.restart_delay_secs, 10);
    }

    #[test]
    fn test_watch_trigger_config_default_delay() {
        let json = r#"{"plugin":"gws","command":"gmail +watch"}"#;
        let cfg: WatchTriggerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.restart_delay_secs, 5);
    }
}
