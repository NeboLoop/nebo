use std::collections::HashSet;

use ai::ToolDefinition;
use db::models::ChatMessage;

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

/// Keyword groups for activating deferred tools.
/// Maps tool name → keywords that should activate the full tool.
const DEFERRED_TOOL_KEYWORDS: &[(&str, &[&str])] = &[
    ("loop", &[
        "neboloop", "channel", "dm", "direct message", "group chat",
        "topic", "broadcast", "send to", "other bot", "message bot",
    ]),
    ("work", &[
        "workflow", "automate", "automation", "procedure", "run workflow",
    ]),
    ("execute", &[
        "run script", "execute script", "python", "node", "javascript",
        "run code", "execute code", "script",
    ]),
    ("plugin", &[
        "plugin", "gmail", "google workspace", "gws", "gdrive",
        "google calendar", "google docs", "google sheets",
    ]),
    ("publisher", &[
        "publish", "marketplace", "submit skill", "submit agent",
        "developer account", "upload binary",
    ]),
];

/// Detect which deferred tools should be activated based on conversation keywords
/// and tools that have already been called.
pub fn detect_deferred_activations(
    messages: &[ChatMessage],
    called_tools: &[String],
    deferred_names: &HashSet<String>,
    already_activated: &HashSet<String>,
) -> HashSet<String> {
    let mut activations = HashSet::new();

    // Any deferred tool that was already called → activate
    for name in called_tools {
        if deferred_names.contains(name) && !already_activated.contains(name) {
            activations.insert(name.clone());
        }
    }

    // MCP proxy tools that were called → activate
    for name in called_tools {
        if name.starts_with("mcp__") && deferred_names.contains(name) && !already_activated.contains(name) {
            activations.insert(name.clone());
        }
    }

    // Keyword matching for known deferred tools
    let recent_text: String = messages
        .iter()
        .rev()
        .filter(|m| m.role == "user" || m.role == "assistant")
        .take(5)
        .map(|m| m.content.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");

    for (tool_name, keywords) in DEFERRED_TOOL_KEYWORDS {
        if !deferred_names.contains(*tool_name) || already_activated.contains(*tool_name) {
            continue;
        }
        if keywords.iter().any(|kw| recent_text.contains(kw)) {
            activations.insert(tool_name.to_string());
        }
    }

    activations
}

/// Detect active contexts based on conversation content.
/// All tools are always included — contexts only control STRAP sub-doc injection.
pub fn filter_tools_with_context(
    all_tools: &[ToolDefinition],
    messages: &[ChatMessage],
    called_tools: &[String],
) -> (Vec<ToolDefinition>, Vec<String>) {
    if all_tools.is_empty() {
        return (vec![], vec![]);
    }

    let mut active_contexts = Vec::new();

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
        }
    }

    // Always return all tools — never filter
    (all_tools.to_vec(), active_contexts)
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
    fn test_all_tools_always_included() {
        let tools = vec![
            make_tool("os"), make_tool("web"), make_tool("agent"),
            make_tool("role"), make_tool("loop"), make_tool("work"),
            make_tool("execute"), make_tool("emit"),
        ];
        let result = filter_tools(&tools, &[], &[]);
        assert_eq!(result.len(), tools.len(), "all tools must always be returned");
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
    fn test_all_tools_returned_regardless_of_keywords() {
        let tools = vec![make_tool("os"), make_tool("loop"), make_tool("work"), make_tool("execute")];
        // No keywords — all tools still returned
        let result = filter_tools(&tools, &[], &[]);
        assert_eq!(result.len(), tools.len());
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

    #[test]
    fn test_deferred_activation_by_keyword() {
        let deferred: HashSet<String> = ["loop", "work", "execute", "plugin"]
            .iter().map(|s| s.to_string()).collect();
        let activated = HashSet::new();
        let messages = vec![make_msg("user", "Send a direct message to the other bot")];
        let result = detect_deferred_activations(&messages, &[], &deferred, &activated);
        assert!(result.contains("loop"), "loop should activate on 'direct message'");
        assert!(!result.contains("work"), "work should not activate without workflow keywords");
    }

    #[test]
    fn test_deferred_activation_by_called_tool() {
        let deferred: HashSet<String> = ["execute", "work"]
            .iter().map(|s| s.to_string()).collect();
        let activated = HashSet::new();
        let result = detect_deferred_activations(&[], &["execute".to_string()], &deferred, &activated);
        assert!(result.contains("execute"), "called deferred tool should activate it");
    }

    #[test]
    fn test_deferred_already_activated_skipped() {
        let deferred: HashSet<String> = ["loop"].iter().map(|s| s.to_string()).collect();
        let mut activated = HashSet::new();
        activated.insert("loop".to_string());
        let messages = vec![make_msg("user", "Send a direct message")];
        let result = detect_deferred_activations(&messages, &[], &deferred, &activated);
        assert!(result.is_empty(), "already activated tools should not re-activate");
    }

    #[test]
    fn test_deferred_plugin_keyword() {
        let deferred: HashSet<String> = ["plugin"].iter().map(|s| s.to_string()).collect();
        let activated = HashSet::new();
        let messages = vec![make_msg("user", "Check my gmail for new messages")];
        let result = detect_deferred_activations(&messages, &[], &deferred, &activated);
        assert!(result.contains("plugin"), "plugin should activate on 'gmail'");
    }
}
