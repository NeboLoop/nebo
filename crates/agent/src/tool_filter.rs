use ai::ToolDefinition;
use db::models::ChatMessage;

/// Core tools that are always included regardless of context.
const CORE_TOOLS: &[&str] = &["os", "web", "agent", "event", "message", "skill", "role"];

/// Contextual tool groups: (context_name, trigger_keywords).
/// Context names map to STRAP sub-docs and/or tool names.
/// For "os" sub-contexts (desktop, app, music, etc.), the tool is always registered
/// but the STRAP docs are only injected when keywords match.
const CONTEXTUAL_GROUPS: &[(&str, &[&str])] = &[
    // Web & browsing (also core, keywords for adjacency)
    ("web", &[
        "browse", "website", "url", "http", "fetch", "search", "google",
        "look up", "find out", "internet", "link", "page", "navigate",
        "yelp", "reddit", "youtube", "wiki",
    ]),
    // Scheduling & events (also core, keywords for adjacency)
    ("event", &[
        "event", "schedule", "remind", "alarm", "timer", "in 10 minutes",
        "every day", "cron", "recurring", "later", "tomorrow", "next week",
        "at 5pm", "in an hour", "daily", "weekly", "monthly",
    ]),
    // NeboLoop communication (tool: loop)
    ("loop", &[
        "neboloop", "channel", "dm", "direct message", "group chat",
        "topic", "broadcast", "send to",
    ]),
    // Workflows (tool: work)
    ("work", &[
        "workflow", "automate", "automation", "procedure", "run workflow",
    ]),
    // Desktop GUI automation (os sub-context)
    ("desktop", &[
        "click", "mouse", "keyboard", "window", "screenshot", "screen",
        "capture", "visible", "see what", "gui", "menu", "dialog",
        "accessibility", "drag", "scroll", "type in", "hotkey",
        "tts", "speak", "say aloud", "dock", "virtual desktop",
    ]),
    // App lifecycle (os sub-context)
    ("app", &[
        "launch", "open app", "close app", "running app", "activate",
        "frontmost", "switch to", "quit app", "start app", "which app",
    ]),
    // Personal information management (os sub-context)
    ("organizer", &[
        "calendar", "reminder", "contact", "email", "schedule",
        "appointment", "mail", "inbox", "unread",
    ]),
    // Music & media (os sub-context)
    ("music", &[
        "music", "play", "pause", "song", "playlist", "track", "album",
        "shuffle", "next song", "spotify", "now playing", "what's playing",
    ]),
    // System settings (os sub-context)
    ("settings", &[
        "volume", "brightness", "wifi", "bluetooth", "dark mode", "mute",
        "unmute", "sleep", "lock screen", "battery", "system info",
    ]),
    // Credential storage (os sub-context)
    ("keychain", &[
        "password", "credential", "keychain", "secret", "api key",
        "token", "login", "stored password",
    ]),
    // File search (os sub-context)
    ("spotlight", &[
        "find file", "search file", "locate", "spotlight", "search for file",
        "where is the file", "mdfind",
    ]),
    // Script execution (tool: execute)
    ("execute", &[
        "run script", "execute script", "python", "node", "javascript",
        "run code", "execute code",
    ]),
    // Event emission (tool: emit)
    ("emit", &[
        "emit event", "fire event", "trigger event",
    ]),
];

/// Context names that correspond to actual registered tools (not os sub-contexts).
const TOOL_CONTEXTS: &[&str] = &["web", "event", "loop", "work", "execute", "emit"];

/// Filter tools based on conversation context.
/// Returns filtered tools AND active context names (for STRAP sub-doc injection).
pub fn filter_tools_with_context(
    all_tools: &[ToolDefinition],
    messages: &[ChatMessage],
    called_tools: &[String],
) -> (Vec<ToolDefinition>, Vec<String>) {
    if all_tools.is_empty() {
        return (vec![], vec![]);
    }

    let mut included_tools = std::collections::HashSet::new();
    let mut active_contexts = Vec::new();

    // Always include core tools
    for name in CORE_TOOLS {
        included_tools.insert(name.to_string());
    }

    // Check recent messages (last 5) for contextual keywords
    let recent_text: String = messages
        .iter()
        .rev()
        .filter(|m| m.role == "user" || m.role == "assistant")
        .take(5)
        .map(|m| m.content.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");

    for (context_name, keywords) in CONTEXTUAL_GROUPS {
        let matched = keywords.iter().any(|kw| recent_text.contains(kw))
            || called_tools.iter().any(|ct| {
                // Match on tool name (for tool contexts like "loop", "work")
                ct == *context_name
                // Also match "os" called_tool for any os sub-context
                || (ct == "os" && !TOOL_CONTEXTS.contains(context_name))
            });

        if matched {
            active_contexts.push(context_name.to_string());

            // If this context is a tool (not an os sub-context), include it
            if TOOL_CONTEXTS.contains(context_name) {
                included_tools.insert(context_name.to_string());
            }
        }
    }

    // Filter tools by included set
    let result: Vec<ToolDefinition> = all_tools
        .iter()
        .filter(|t| included_tools.contains(&t.name))
        .cloned()
        .collect();

    // Never return empty — fall back to all tools
    let tools = if result.is_empty() {
        all_tools.to_vec()
    } else {
        result
    };

    (tools, active_contexts)
}

/// Backward-compatible filter that discards contexts.
pub fn filter_tools(
    all_tools: &[ToolDefinition],
    messages: &[ChatMessage],
    called_tools: &[String],
) -> Vec<ToolDefinition> {
    filter_tools_with_context(all_tools, messages, called_tools).0
}

/// Get the names of tools that would pass the filter.
pub fn active_tool_names(
    all_tool_names: &[String],
    messages: &[ChatMessage],
    called_tools: &[String],
) -> Vec<String> {
    let defs: Vec<ToolDefinition> = all_tool_names
        .iter()
        .map(|n| ToolDefinition {
            name: n.clone(),
            description: String::new(),
            input_schema: serde_json::json!({}),
        })
        .collect();

    filter_tools(&defs, messages, called_tools)
        .into_iter()
        .map(|t| t.name)
        .collect()
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

    fn make_msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            id: String::new(),
            chat_id: String::new(),
            role: role.to_string(),
            content: content.to_string(),
            metadata: None,
            created_at: 0,
            day_marker: None,
            tool_calls: None,
            tool_results: None,
            token_estimate: None,
        }
    }

    #[test]
    fn test_core_tools_always_included() {
        let tools = vec![make_tool("os"), make_tool("web"), make_tool("agent"), make_tool("role")];
        let result = filter_tools(&tools, &[], &[]);
        let names: Vec<&str> = result.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"os"));
        assert!(names.contains(&"web"));
        assert!(names.contains(&"agent"));
        assert!(names.contains(&"role"));
    }

    #[test]
    fn test_contextual_keyword_activates_context() {
        let tools = vec![make_tool("os"), make_tool("web")];
        let messages = vec![make_msg("user", "Take a screenshot of the current screen")];
        let (result, contexts) = filter_tools_with_context(&tools, &messages, &[]);
        // os is always included (core), "desktop" context should activate
        assert!(result.iter().any(|t| t.name == "os"));
        assert!(contexts.contains(&"desktop".to_string()));
    }

    #[test]
    fn test_music_keyword_activates_context() {
        let tools = vec![make_tool("os")];
        let messages = vec![make_msg("user", "Play some music")];
        let (_, contexts) = filter_tools_with_context(&tools, &messages, &[]);
        assert!(contexts.contains(&"music".to_string()));
    }

    #[test]
    fn test_loop_keyword_includes_tool() {
        let tools = vec![make_tool("os"), make_tool("loop")];
        let messages = vec![make_msg("user", "Send a dm to the other bot")];
        let result = filter_tools(&tools, &messages, &[]);
        let names: Vec<&str> = result.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"loop"));
    }

    #[test]
    fn test_os_adjacency_activates_sub_contexts() {
        let tools = vec![make_tool("os")];
        let (_, contexts) = filter_tools_with_context(&tools, &[], &["os".to_string()]);
        // "os" in called_tools should activate os sub-contexts
        assert!(contexts.contains(&"desktop".to_string()));
        assert!(contexts.contains(&"music".to_string()));
    }

    #[test]
    fn test_organizer_keyword() {
        let tools = vec![make_tool("os")];
        let messages = vec![make_msg("user", "Check my calendar for today")];
        let (_, contexts) = filter_tools_with_context(&tools, &messages, &[]);
        assert!(contexts.contains(&"organizer".to_string()));
    }

    #[test]
    fn test_keychain_keyword() {
        let tools = vec![make_tool("os")];
        let messages = vec![make_msg("user", "What's my github password?")];
        let (_, contexts) = filter_tools_with_context(&tools, &messages, &[]);
        assert!(contexts.contains(&"keychain".to_string()));
    }

    #[test]
    fn test_active_tool_names() {
        let all = vec!["os".to_string(), "web".to_string(), "agent".to_string()];
        let messages = vec![make_msg("user", "Play some music")];
        let names = active_tool_names(&all, &messages, &[]);
        assert!(names.contains(&"os".to_string())); // core
        assert!(names.contains(&"agent".to_string())); // core
    }
}
