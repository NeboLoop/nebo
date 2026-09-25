//! The prompt's inputs resolved from the employee and the workspace, once per
//! turn: the AGENT.md body, the employee's own setup, the plugins its job
//! needs and the workspace notes. `run_loop` builds its prompt from the same
//! functions until the cutover deletes it.

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
) -> String {
    let Some(cfg) = agent.config.as_ref() else {
        return String::new();
    };
    let mut required = cfg.requires.plugins.clone();
    // Merge scope-specific plugins
    if let Some(scope) = tool_scope.and_then(|name| cfg.scopes.get(name)) {
        for p in &scope.plugins {
            if !required.contains(p) {
                required.push(p.clone());
            }
        }
    }
    skill_loader
        .map(|l| l.agent_plugin_context(&required))
        .unwrap_or_default()
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
            "These skills are part of your configuration. Use skill(action: \"discover\", query: \"...\") to find one and skill(action: \"load\", name: \"...\") to read it."
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
