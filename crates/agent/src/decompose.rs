use tracing::debug;

use crate::task_graph::{AgentType, TaskNode, TaskStatus};

const DECOMPOSE_PROMPT: &str = r#"Break this task into independent sub-tasks. Minimize dependencies — maximize parallelism.

Respond with ONLY a JSON array, no markdown:
[
  {"id": "1", "description": "short desc", "prompt": "detailed instructions", "agent_type": "explore", "depends_on": []},
  {"id": "2", "description": "short desc", "prompt": "detailed instructions", "agent_type": "general", "depends_on": ["1"]}
]

Rules:
- "explore" agents: research, search, read files — READ ONLY, no modifications
- "plan" agents: analyze, design, compare — produce plans, not actions
- "general" agents: full tool access — create, modify, execute
- If a task needs output from another, add it to depends_on
- Tasks with empty depends_on run immediately and concurrently
- Keep each task focused: one clear objective per task
- If the task is simple (single step), return a single-element array

Task: "#;

/// Decompose a complex task into a list of TaskNodes with one model call.
pub async fn decompose_task(provider: &dyn ai::Provider, trace: ai::RequestTrace, prompt: &str) -> Result<Vec<TaskNode>, String> {
    let request = ai::ChatRequest {
        tool_credential: None,
        tool_choice: Default::default(),
        messages: vec![ai::Message {
            role: "user".to_string(),
            content: format!("{DECOMPOSE_PROMPT}{prompt}"),
            ..Default::default()
        }],
        tools: vec![],
        max_tokens: 4096,
        temperature: 0.7,
        system: String::new(),
        static_system: String::new(),
        model: String::new(),
        enable_thinking: false,
        metadata: None,
        cache_breakpoints: vec![],
        cancel_token: None,
        trace,
    };
    let mut rx = provider
        .stream(&request)
        .await
        .map_err(|e| format!("Decomposition LLM call failed: {e}"))?;
    let mut response = String::new();
    while let Some(event) = rx.recv().await {
        if event.event_type == ai::StreamEventType::Text {
            response.push_str(&event.text);
        }
    }
    parse_decomposition(&response)
}

/// Parse the LLM's JSON response into TaskNodes.
fn parse_decomposition(response: &str) -> Result<Vec<TaskNode>, String> {
    let trimmed = response.trim();

    // Strip markdown fences if present
    let json_str = if trimmed.starts_with("```") {
        let inner = trimmed
            .trim_start_matches("```json")
            .trim_start_matches("```");
        inner.trim_end_matches("```").trim()
    } else {
        trimmed
    };

    // Try to find JSON array in the response
    let json_str = if let Some(start) = json_str.find('[') {
        if let Some(end) = json_str.rfind(']') {
            &json_str[start..=end]
        } else {
            json_str
        }
    } else {
        json_str
    };

    let raw: Vec<RawTask> = serde_json::from_str(json_str).map_err(|e| {
        format!(
            "Failed to parse decomposition JSON: {} — response: {}",
            e,
            &response[..response.len().min(200)]
        )
    })?;

    if raw.is_empty() {
        return Err("Decomposition returned empty task list".to_string());
    }

    let nodes: Vec<TaskNode> = raw
        .into_iter()
        .map(|t| TaskNode {
            id: t.id,
            prompt: t.prompt,
            description: t.description,
            agent_type: AgentType::from_str(&t.agent_type),
            model_override: String::new(),
            depends_on: t.depends_on,
            status: TaskStatus::Pending,
            result: None,
            error: None,
        })
        .collect();

    debug!(count = nodes.len(), "decomposed task into sub-tasks");
    Ok(nodes)
}

#[derive(serde::Deserialize)]
struct RawTask {
    id: String,
    description: String,
    prompt: String,
    #[serde(default = "default_agent_type")]
    agent_type: String,
    #[serde(default)]
    depends_on: Vec<String>,
}

fn default_agent_type() -> String {
    "general".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_clean_json() {
        let json = r#"[
            {"id": "1", "description": "Research X", "prompt": "Find info about X", "agent_type": "explore", "depends_on": []},
            {"id": "2", "description": "Analyze", "prompt": "Analyze findings", "agent_type": "general", "depends_on": ["1"]}
        ]"#;

        let nodes = parse_decomposition(json).unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].agent_type, AgentType::Explore);
        assert_eq!(nodes[1].depends_on, vec!["1"]);
    }

    #[test]
    fn test_parse_markdown_fenced() {
        let json = "```json\n[\n{\"id\": \"1\", \"description\": \"Task\", \"prompt\": \"Do it\", \"agent_type\": \"general\", \"depends_on\": []}\n]\n```";

        let nodes = parse_decomposition(json).unwrap();
        assert_eq!(nodes.len(), 1);
    }

    #[test]
    fn test_parse_with_surrounding_text() {
        let json = "Here are the tasks:\n[{\"id\": \"1\", \"description\": \"Task\", \"prompt\": \"Do it\", \"agent_type\": \"general\", \"depends_on\": []}]\nDone!";

        let nodes = parse_decomposition(json).unwrap();
        assert_eq!(nodes.len(), 1);
    }

    #[test]
    fn test_parse_empty_array() {
        let result = parse_decomposition("[]");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[test]
    fn test_parse_wide_plan_is_not_capped() {
        // Width is ours (auditor Rule 15): a plan is never refused for its
        // task count — the LLM permit pool is the brake, not the parser.
        let tasks: Vec<String> = (1..=40)
            .map(|i| {
                format!(
                    r#"{{"id": "{}", "description": "Task {}", "prompt": "Do {}", "agent_type": "general", "depends_on": []}}"#,
                    i, i, i
                )
            })
            .collect();
        let json = format!("[{}]", tasks.join(","));

        let nodes = parse_decomposition(&json).expect("40 independent tasks parse");
        assert_eq!(nodes.len(), 40);
    }

    #[test]
    fn test_parse_missing_agent_type_defaults() {
        let json = r#"[{"id": "1", "description": "Task", "prompt": "Do it", "depends_on": []}]"#;
        let nodes = parse_decomposition(json).unwrap();
        assert_eq!(nodes[0].agent_type, AgentType::General);
    }

    #[test]
    fn test_malformed_json() {
        let result = parse_decomposition("not json at all");
        assert!(result.is_err());
    }
}
