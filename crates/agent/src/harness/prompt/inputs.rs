//! What the turn's identity and session-context rows are built from,
//! resolved from the employee and the workspace once per turn: the AGENT.md
//! body, the employee's own setup, the plugins and tools its job uses and
//! the workspace notes.

use tracing::{debug, warn};

/// The AGENT.md body the prompt carries: the prose, under a compact identity
/// header (name, description, triggers) from its frontmatter.
pub fn persona_body(agent_md: &str) -> String {
    // Strip YAML frontmatter from AGENT.md — inject only the prose body.
    // Frontmatter is machine metadata (name, triggers, etc.), not persona instructions.
    match napp::agent::split_frontmatter(agent_md) {
        Ok((yaml_str, body)) => {
            if yaml_str.is_empty() {
                body
            } else {
                // Include a compact identity header from frontmatter properties
                let mut result = String::new();
                if let Ok(mapping) = serde_yaml::from_str::<serde_yaml::Mapping>(&yaml_str) {
                    let mut identity_parts = Vec::new();
                    for (k, v) in &mapping {
                        if let (serde_yaml::Value::String(key), val) = (k, v) {
                            match key.as_str() {
                                "name" | "description" | "triggers" => {
                                    let val_str = match val {
                                        serde_yaml::Value::String(s) => s.clone(),
                                        serde_yaml::Value::Sequence(seq) => seq
                                            .iter()
                                            .filter_map(|i| match i {
                                                serde_yaml::Value::String(s) => {
                                                    Some(s.as_str())
                                                }
                                                _ => None,
                                            })
                                            .collect::<Vec<_>>()
                                            .join(", "),
                                        _ => continue,
                                    };
                                    identity_parts.push(format!("- **{}**: {}", key, val_str));
                                }
                                _ => {}
                            }
                        }
                    }
                    if !identity_parts.is_empty() {
                        result.push_str(&identity_parts.join("\n"));
                        result.push_str("\n\n");
                    }
                }
                result.push_str(&body);
                result
            }
        }
        Err(_) => agent_md.to_string(),
    }
}

/// What the plugins the employee's job needs do and which skills they carry,
/// the active tool scope's plugins included.
pub fn plugin_context(
    agent: &tools::ActiveAgent,
    tool_scope: Option<&str>,
    skill_loader: Option<&tools::skills::Loader>,
    store: &db::Store,
) -> String {
    let slugs = required_plugins(agent, tool_scope, store);
    skill_loader
        .map(|l| l.agent_plugin_context(&slugs))
        .unwrap_or_default()
}

/// The installed plugins the employee's job requires (`requires.plugins`,
/// the active tool scope's plugins included), as slugs, whether the job
/// names each by slug, qualified name or install code.
fn required_plugins(
    agent: &tools::ActiveAgent,
    tool_scope: Option<&str>,
    store: &db::Store,
) -> Vec<String> {
    let Some(cfg) = agent.config.as_ref() else {
        return Vec::new();
    };
    let scope_plugins = tool_scope
        .and_then(|s| cfg.scopes.get(s))
        .map(|s| s.plugins.as_slice())
        .unwrap_or_default();
    let mut slugs = Vec::new();
    for reference in cfg.requires.plugins.iter().chain(scope_plugins) {
        if let Some(slug) = tools::plugin_tools::plugin_slug_of(store, reference)
            && !slugs.contains(&slug)
        {
            slugs.push(slug);
        }
    }
    slugs
}

/// The tools the employee's job uses: its `requires.tools`, its required
/// plugins' tools (the active tool scope's included), the operation tools
/// of the interfaces it binds and its own app tools. They are deferred like
/// every tool outside the core set, so the declared tools are the same for
/// every employee; this names them so the employee loads them with
/// find_tools. Only tools registered and deferred now are named, and none
/// the tool scope leaves out (`withheld`).
pub async fn job_tools(
    agent: &tools::ActiveAgent,
    tool_scope: Option<&str>,
    registry: &tools::Registry,
    store: &db::Store,
    withheld: &std::collections::HashSet<String>,
) -> String {
    let mut names: std::collections::BTreeSet<String> = registry.agent_tool_names(&agent.agent_id).await.into_iter().collect();
    if let Some(cfg) = agent.config.as_ref() {
        names.extend(cfg.requires.tools.iter().cloned());
        for slug in required_plugins(agent, tool_scope, store) {
            names.insert(tools::plugin_tools::plugin_tool_name(&slug));
        }
        names.extend(registry.operation_tools_for(&cfg.requires.interfaces).await);
    }
    let deferred = registry.get_deferred_names().await;
    names.retain(|n| deferred.contains(n) && !withheld.contains(n));
    if names.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = names.iter().map(|n| format!("- {n}")).collect();
    format!(
        "## Tools for your job\n{}\n\nThese are in the deferred tool listing; load them with {} before calling.",
        lines.join("\n"),
        tools::find_tools::FIND_TOOLS
    )
}

/// The employee's own setup as it knows itself from its first step: its
/// workflows, its skills and its custom tools.
pub fn self_context(agent: &tools::ActiveAgent) -> String {
    let Some(cfg) = agent.config.as_ref() else {
        return String::new();
    };
    let mut parts = Vec::new();

    // Workflows
    if !cfg.workflows.is_empty() {
        let mut wf_lines = vec![format!("## Your Workflows ({})\n", cfg.workflows.len())];
        let mut sorted: Vec<_> = cfg.workflows.iter().collect();
        sorted.sort_by_key(|(name, _)| name.as_str());
        for (name, binding) in &sorted {
            let trigger_desc = match &binding.trigger {
                napp::agent::AgentTrigger::Schedule { schedule, cron, .. } => {
                    if let Some(s) = schedule {
                        format!("schedule: {}", s)
                    } else {
                        format!("schedule: {}", cron)
                    }
                }
                napp::agent::AgentTrigger::Heartbeat { interval, window } => {
                    if let Some(w) = window {
                        format!("heartbeat: every {} within {}", interval, w)
                    } else {
                        format!("heartbeat: every {}", interval)
                    }
                }
                napp::agent::AgentTrigger::Event { sources } => {
                    format!("event: {}", sources.join(", "))
                }
                napp::agent::AgentTrigger::Watch { plugin, event, .. } => {
                    if let Some(ev) = event {
                        format!("watch: {}.{}", plugin, ev)
                    } else {
                        format!("watch: {}", plugin)
                    }
                }
                napp::agent::AgentTrigger::Folder { path, .. } => {
                    format!("folder: {}", path)
                }
                napp::agent::AgentTrigger::Manual => "manual".to_string(),
                napp::agent::AgentTrigger::Call { line } => {
                    format!(
                        "call tree for the {} phone line",
                        if line.is_empty() { "every" } else { line }
                    )
                }
            };
            let desc = if binding.description.is_empty() {
                String::new()
            } else {
                format!(" — {}", binding.description)
            };
            let activity_count = binding.activities.len();
            wf_lines.push(format!(
                "- **{}**{} [{}] ({} activities)",
                name, desc, trigger_desc, activity_count
            ));
        }
        wf_lines.push(String::new());
        wf_lines.push(
            "Start a workflow by hand with run_workflow; workflow_status shows its last run."
                .to_string(),
        );
        parts.push(wf_lines.join("\n"));
    }

    // Skills declared by this agent
    if !cfg.skills.is_empty() {
        let mut sk_lines = vec![format!("## Your Skills ({})\n", cfg.skills.len())];
        for skill_ref in &cfg.skills {
            sk_lines.push(format!("- {}", skill_ref));
        }
        sk_lines.push(String::new());
        sk_lines.push(
            "These skills are part of your job. They are in the skill listing; load one with use_skill when the work calls for it."
                .to_string(),
        );
        parts.push(sk_lines.join("\n"));
    }

    // Sidecar tools (custom HTTP endpoint tools defined by this agent)
    if !cfg.tools.is_empty() {
        let mut tool_lines = vec![format!("## Your Custom Tools ({})\n", cfg.tools.len())];
        for tool_def in &cfg.tools {
            tool_lines.push(format!(
                "- **{}** — {}",
                tool_def.name, tool_def.description
            ));
        }
        parts.push(tool_lines.join("\n"));
    }

    parts.join("\n\n")
}

/// Load workspace context from `.nebo.md` or `NEBO.md`.
/// Walks up from CWD to git root (or home dir), returns the first match.
pub fn workspace_notes() -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    let mut dir = cwd.as_path();

    loop {
        for name in &[".nebo.md", "NEBO.md"] {
            let path = dir.join(name);
            if path.is_file() {
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        let sanitized = crate::sanitize::sanitize_for_prompt(&content);
                        debug!(path = %path.display(), "loaded workspace context file");
                        return Some(sanitized);
                    }
                    Err(e) => {
                        warn!(path = %path.display(), error = %e, "failed to read context file");
                    }
                }
            }
        }

        // Stop at git root
        if dir.join(".git").exists() {
            break;
        }

        // Walk up
        match dir.parent() {
            Some(parent) if parent != dir => dir = parent,
            _ => break,
        }
    }

    None
}
