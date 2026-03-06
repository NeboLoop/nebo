use ai::ToolDefinition;
use db::models::ChatMessage;

/// Core tools that are always included regardless of context.
const CORE_TOOLS: &[&str] = &["system", "web", "bot", "loop", "event", "message", "skill", "tool", "work"];

/// Contextual tool groups: (tool_name, trigger_keywords).
const CONTEXTUAL_GROUPS: &[(&str, &[&str])] = &[
    ("screenshot", &["screenshot", "screen", "capture", "visible", "see what"]),
    ("vision", &["image", "photo", "picture", "screenshot", "visual"]),
    ("desktop", &["click", "type", "mouse", "keyboard", "window", "app", "open"]),
    ("organizer", &["calendar", "reminder", "contact", "email", "schedule"]),
];

/// Filter tools based on conversation context.
/// Core tools are always included. Contextual tools are included when
/// recent messages mention relevant keywords or when any tool in their
/// group was recently called.
pub fn filter_tools(
    all_tools: &[ToolDefinition],
    messages: &[ChatMessage],
    called_tools: &[String],
) -> Vec<ToolDefinition> {
    if all_tools.is_empty() {
        return vec![];
    }

    let mut included = std::collections::HashSet::new();

    // Always include core tools
    for name in CORE_TOOLS {
        included.insert(name.to_string());
    }

    // Check recent messages (last 5) for contextual keywords
    let recent_messages: Vec<&ChatMessage> = messages
        .iter()
        .rev()
        .filter(|m| m.role == "user" || m.role == "assistant")
        .take(5)
        .collect();

    let recent_text: String = recent_messages
        .iter()
        .map(|m| m.content.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");

    for (tool_name, keywords) in CONTEXTUAL_GROUPS {
        // Include if keywords match
        if keywords.iter().any(|kw| recent_text.contains(kw)) {
            included.insert(tool_name.to_string());
            continue;
        }

        // Include if any tool in the group was recently called (adjacency)
        if called_tools.iter().any(|ct| ct == *tool_name) {
            included.insert(tool_name.to_string());
        }
    }

    // Filter and return
    let result: Vec<ToolDefinition> = all_tools
        .iter()
        .filter(|t| included.contains(&t.name))
        .cloned()
        .collect();

    // Never return empty — fall back to all tools
    if result.is_empty() {
        return all_tools.to_vec();
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: String::new(),
            input_schema: serde_json::json!({}),
        }
    }

    #[test]
    fn test_core_tools_always_included() {
        let tools = vec![make_tool("system"), make_tool("web"), make_tool("screenshot")];
        let result = filter_tools(&tools, &[], &[]);
        let names: Vec<&str> = result.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"system"));
        assert!(names.contains(&"web"));
        assert!(!names.contains(&"screenshot")); // Not a core tool, no keyword match
    }

    #[test]
    fn test_contextual_tool_by_keyword() {
        let tools = vec![make_tool("system"), make_tool("screenshot")];
        let messages = vec![ChatMessage {
            id: String::new(),
            chat_id: String::new(),
            role: "user".to_string(),
            content: "Take a screenshot of the current screen".to_string(),
            metadata: None,
            created_at: 0,
            day_marker: None,
            tool_calls: None,
            tool_results: None,
            token_estimate: None,
        }];
        let result = filter_tools(&tools, &messages, &[]);
        let names: Vec<&str> = result.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"screenshot"));
    }
}
