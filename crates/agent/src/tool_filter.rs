use std::collections::HashSet;

use ai::ToolDefinition;
use db::models::ChatMessage;

/// Contextual tool groups: (context_name, trigger_keywords).
/// Context names map to STRAP sub-docs and/or tool names.
/// For "os" sub-contexts (desktop, app, music, etc.), the tool is always registered
/// but the STRAP docs are only injected when keywords match.
const CONTEXTUAL_GROUPS: &[(&str, &[&str])] = &[
    // Web & browsing (also core, keywords for adjacency)
    (
        "web",
        &[
            "browse", "website", "url", "http", "fetch", "search", "google", "look up", "find out",
            "internet", "link", "page", "navigate", "yelp", "reddit", "youtube", "wiki",
        ],
    ),
    // Scheduling & events (also core, keywords for adjacency)
    (
        "event",
        &[
            "event",
            "schedule",
            "remind",
            "alarm",
            "timer",
            "in 10 minutes",
            "every day",
            "cron",
            "recurring",
            "later",
            "tomorrow",
            "next week",
            "at 5pm",
            "in an hour",
            "daily",
            "weekly",
            "monthly",
        ],
    ),
    // NeboLoop communication (tool: loop)
    (
        "loop",
        &[
            "neboloop",
            "channel",
            "dm",
            "direct message",
            "group chat",
            "topic",
            "broadcast",
            "send to",
        ],
    ),
    // Workflows (tool: work)
    (
        "work",
        &[
            "workflow",
            "automate",
            "automation",
            "procedure",
            "run workflow",
        ],
    ),
    // Desktop GUI automation (os sub-context)
    (
        "desktop",
        &[
            "click",
            "mouse",
            "keyboard",
            "window",
            "screenshot",
            "screen",
            "capture",
            "visible",
            "see what",
            "gui",
            "menu",
            "dialog",
            "accessibility",
            "drag",
            "scroll",
            "type in",
            "hotkey",
            "tts",
            "speak",
            "say aloud",
            "dock",
            "virtual desktop",
        ],
    ),
    // App lifecycle (os sub-context)
    (
        "app",
        &[
            "launch",
            "open app",
            "close app",
            "running app",
            "activate",
            "frontmost",
            "switch to",
            "quit app",
            "start app",
            "which app",
        ],
    ),
    // Personal information management (os sub-context)
    (
        "organizer",
        &[
            "calendar",
            "reminder",
            "contact",
            "email",
            "schedule",
            "appointment",
            "mail",
            "inbox",
            "unread",
            "invite",
            "invitation",
            "rsvp",
        ],
    ),
    // Music & media (os sub-context)
    (
        "music",
        &[
            "music",
            "play",
            "pause",
            "song",
            "playlist",
            "track",
            "album",
            "shuffle",
            "next song",
            "spotify",
            "now playing",
            "what's playing",
        ],
    ),
    // System settings (os sub-context)
    (
        "settings",
        &[
            "volume",
            "brightness",
            "wifi",
            "bluetooth",
            "dark mode",
            "mute",
            "unmute",
            "sleep",
            "lock screen",
            "battery",
            "system info",
        ],
    ),
    // Credential storage (os sub-context)
    (
        "keychain",
        &[
            "password",
            "credential",
            "keychain",
            "secret",
            "api key",
            "token",
            "login",
            "stored password",
        ],
    ),
    // File search (os sub-context)
    (
        "spotlight",
        &[
            "find file",
            "search file",
            "locate",
            "spotlight",
            "search for file",
            "where is the file",
            "mdfind",
        ],
    ),
    // Script execution (tool: execute)
    (
        "execute",
        &[
            "run script",
            "execute script",
            "python",
            "node",
            "javascript",
            "run code",
            "execute code",
        ],
    ),
    // Event emission (tool: emit)
    ("emit", &["emit event", "fire event", "trigger event"]),
];

/// Context names that correspond to actual registered tools (not os sub-contexts).
const TOOL_CONTEXTS: &[&str] = &["web", "event", "loop", "work", "execute", "emit"];

/// Tools always included in the schema list regardless of context.
/// These are core agent capabilities that should never be filtered out.
const ALWAYS_INCLUDE_TOOLS: &[&str] = &["agent", "skill", "event", "message", "tool_search"];

// Keyword-based deferred activation removed. Tools now load and unload via
// message-history scanning (extract_discovered_deferred_tools), following
// Claude Code's pattern. Model explicitly calls tool_search to discover tools.

/// Extract deferred tools that are currently "discovered" in the message window.
///
/// Follows Claude Code's `extractDiscoveredToolNames(messages)` pattern:
/// - Scans assistant `tool_calls` for deferred tools that were directly called
/// - Scans tool result messages for `tool_search` responses, extracting `matches`
///
/// The key property: when sliding window evicts messages, any tool_search results
/// or tool calls in those messages disappear, so the tool naturally unloads.
/// Tools come and go with the message window.
pub fn extract_discovered_deferred_tools(
    messages: &[ChatMessage],
    deferred_names: &HashSet<String>,
) -> HashSet<String> {
    let mut discovered = HashSet::new();

    // Collect tool_call_ids from tool_search invocations so we can find their results
    let mut tool_search_call_ids: HashSet<String> = HashSet::new();

    for msg in messages {
        // 1. Scan assistant messages for tool_calls
        if msg.role == "assistant" {
            if let Some(ref tc_json) = msg.tool_calls {
                if let Ok(calls) = serde_json::from_str::<Vec<serde_json::Value>>(tc_json) {
                    for call in &calls {
                        let name = call.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        // Track tool_search call IDs
                        if name == "tool_search" {
                            if let Some(id) = call.get("id").and_then(|v| v.as_str()) {
                                tool_search_call_ids.insert(id.to_string());
                            }
                        }
                        // Direct calls to deferred tools activate them
                        if deferred_names.contains(name) {
                            discovered.insert(name.to_string());
                        }
                    }
                }
            }
        }

        // 2. Scan tool result messages for tool_search responses
        if msg.role == "tool" {
            if let Some(ref tr_json) = msg.tool_results {
                if let Ok(results) = serde_json::from_str::<Vec<serde_json::Value>>(tr_json) {
                    for result in &results {
                        // Match by tool_call_id or by content shape (total_deferred marker)
                        let is_tool_search = result
                            .get("tool_call_id")
                            .and_then(|v| v.as_str())
                            .map(|id| tool_search_call_ids.contains(id))
                            .unwrap_or(false);

                        let content_str =
                            result.get("content").and_then(|v| v.as_str()).unwrap_or("");

                        // Fallback: detect tool_search results by shape if ID matching fails
                        let looks_like_search = !is_tool_search
                            && content_str.contains("total_deferred")
                            && content_str.contains("matches");

                        if is_tool_search || looks_like_search {
                            if let Ok(search) =
                                serde_json::from_str::<serde_json::Value>(content_str)
                            {
                                if let Some(matches) =
                                    search.get("matches").and_then(|v| v.as_array())
                                {
                                    for m in matches {
                                        if let Some(name) = m.as_str() {
                                            if deferred_names.contains(name) {
                                                discovered.insert(name.to_string());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    discovered
}

/// Detect active contexts based on conversation content.
/// All tools are always included — contexts only control STRAP sub-doc injection.
pub fn filter_tools_with_context(
    all_tools: &[ToolDefinition],
    messages: &[ChatMessage],
    called_tools: &[String],
    agent_tool_names: &HashSet<String>,
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

    // Filter tool schemas to only include tools relevant to the current context.
    // Core tools (agent, skill, event, message, tool_search) always pass through.
    // Other tools must match an active context or have been called in this session.
    let active_tool_names: HashSet<&str> = active_contexts
        .iter()
        .filter(|c| TOOL_CONTEXTS.contains(&c.as_str()))
        .map(|c| c.as_str())
        .collect();

    let called_set: HashSet<&str> = called_tools.iter().map(|s| s.as_str()).collect();

    let filtered_tools: Vec<ToolDefinition> = all_tools
        .iter()
        .filter(|tool| {
            let name = tool.name.as_str();
            // Always include core tools
            ALWAYS_INCLUDE_TOOLS.contains(&name)
            // Include tools matching active contexts
            || active_tool_names.contains(name)
            // Include tools that were already called this session
            || called_set.contains(name)
            // Include MCP proxy tools (always pass through)
            || name.starts_with("mcp__")
            // Include the agent's own sidecar tools (native per-endpoint tools)
            || agent_tool_names.contains(name)
            // Include "os" if any OS sub-context matched (desktop, app, music, etc.)
            || (name == "os" && active_contexts.iter().any(|c| !TOOL_CONTEXTS.contains(&c.as_str())))
        })
        .cloned()
        .collect();

    (filtered_tools, active_contexts)
}

/// Backward-compatible filter that discards contexts.
pub fn filter_tools(
    all_tools: &[ToolDefinition],
    messages: &[ChatMessage],
    called_tools: &[String],
) -> Vec<ToolDefinition> {
    filter_tools_with_context(all_tools, messages, called_tools, &HashSet::new()).0
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
            html: None,
        }
    }

    #[test]
    fn test_core_tools_always_included() {
        let tools = vec![
            make_tool("os"),
            make_tool("web"),
            make_tool("agent"),
            make_tool("skill"),
            make_tool("event"),
            make_tool("message"),
            make_tool("tool_search"),
            make_tool("loop"),
            make_tool("work"),
            make_tool("execute"),
            make_tool("emit"),
        ];
        let result = filter_tools(&tools, &[], &[]);
        // Core tools always pass through: agent, skill, event, message, tool_search
        let names: Vec<&str> = result.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"agent"), "agent must always be included");
        assert!(names.contains(&"skill"), "skill must always be included");
        assert!(names.contains(&"event"), "event must always be included");
        assert!(names.contains(&"message"), "message must always be included");
        assert!(names.contains(&"tool_search"), "tool_search must always be included");
        // Non-core tools without matching context are filtered out
        assert!(!names.contains(&"web"), "web should be filtered without keywords");
        assert!(!names.contains(&"loop"), "loop should be filtered without keywords");
    }

    #[test]
    fn test_contextual_keyword_activates_context() {
        let tools = vec![make_tool("os"), make_tool("web")];
        let messages = vec![make_msg("user", "Take a screenshot of the current screen")];
        let (result, contexts) = filter_tools_with_context(&tools, &messages, &[], &HashSet::new());
        // "desktop" sub-context keyword activates os tool
        assert!(result.iter().any(|t| t.name == "os"));
        assert!(contexts.contains(&"desktop".to_string()));
        // web should be filtered out (no web keywords)
        assert!(!result.iter().any(|t| t.name == "web"));
    }

    #[test]
    fn test_music_keyword_activates_context() {
        let tools = vec![make_tool("os")];
        let messages = vec![make_msg("user", "Play some music")];
        let (_, contexts) = filter_tools_with_context(&tools, &messages, &[], &HashSet::new());
        assert!(contexts.contains(&"music".to_string()));
    }

    #[test]
    fn test_context_keywords_activate_tools() {
        let tools = vec![
            make_tool("agent"),
            make_tool("web"),
            make_tool("loop"),
        ];
        // "browse" keyword should activate web context → web tool included
        let messages = vec![make_msg("user", "Browse to example.com")];
        let result = filter_tools(&tools, &messages, &[]);
        let names: Vec<&str> = result.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"agent"), "core tool always included");
        assert!(names.contains(&"web"), "web activated by keyword");
        assert!(!names.contains(&"loop"), "loop not activated without keyword");
    }

    #[test]
    fn test_os_adjacency_activates_sub_contexts() {
        let tools = vec![make_tool("os")];
        let (_, contexts) = filter_tools_with_context(&tools, &[], &["os".to_string()], &HashSet::new());
        // "os" in called_tools should activate os sub-contexts
        assert!(contexts.contains(&"desktop".to_string()));
        assert!(contexts.contains(&"music".to_string()));
    }

    #[test]
    fn test_organizer_keyword() {
        let tools = vec![make_tool("os")];
        let messages = vec![make_msg("user", "Check my calendar for today")];
        let (_, contexts) = filter_tools_with_context(&tools, &messages, &[], &HashSet::new());
        assert!(contexts.contains(&"organizer".to_string()));
    }

    #[test]
    fn test_keychain_keyword() {
        let tools = vec![make_tool("os")];
        let messages = vec![make_msg("user", "What's my github password?")];
        let (_, contexts) = filter_tools_with_context(&tools, &messages, &[], &HashSet::new());
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
    fn test_discovered_via_tool_search_result() {
        let deferred: HashSet<String> = ["plugin", "execute"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // Assistant calls tool_search
        let mut assistant = make_msg("assistant", "");
        assistant.tool_calls = Some(
            r#"[{"id": "tc_1", "name": "tool_search", "input": {"query": "plugin"}}]"#.to_string(),
        );
        // Tool result contains tool_search response
        let mut tool_result = make_msg("tool", "");
        tool_result.tool_results = Some(
            r#"[{"tool_call_id": "tc_1", "content": "{\"matches\": [\"plugin\"], \"total_deferred\": 5}", "is_error": false}]"#.to_string(),
        );
        let messages = vec![assistant, tool_result];
        let result = extract_discovered_deferred_tools(&messages, &deferred);
        assert!(result.contains("plugin"), "plugin discovered via tool_search");
        assert!(!result.contains("execute"), "execute not mentioned in results");
    }

    #[test]
    fn test_discovered_via_direct_call() {
        let deferred: HashSet<String> = ["plugin", "execute"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // Assistant directly calls a deferred tool
        let mut assistant = make_msg("assistant", "");
        assistant.tool_calls =
            Some(r#"[{"id": "tc_1", "name": "plugin", "input": {"action": "exec"}}]"#.to_string());
        let messages = vec![assistant];
        let result = extract_discovered_deferred_tools(&messages, &deferred);
        assert!(result.contains("plugin"), "plugin discovered via direct call");
        assert!(!result.contains("execute"), "execute not called");
    }

    #[test]
    fn test_discovered_empty_when_no_references() {
        let deferred: HashSet<String> = ["plugin", "execute"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // Regular user message — no tool_search, no tool calls
        let messages = vec![make_msg("user", "Check my gmail for new messages")];
        let result = extract_discovered_deferred_tools(&messages, &deferred);
        assert!(result.is_empty(), "no discovery without tool_search or direct call");
    }

    #[test]
    fn test_discovered_unloads_when_messages_evicted() {
        let deferred: HashSet<String> = ["plugin"].iter().map(|s| s.to_string()).collect();
        // Simulate: tool_search result was in window
        let mut assistant = make_msg("assistant", "");
        assistant.tool_calls = Some(
            r#"[{"id": "tc_1", "name": "tool_search", "input": {"query": "plugin"}}]"#.to_string(),
        );
        let mut tool_result = make_msg("tool", "");
        tool_result.tool_results = Some(
            r#"[{"tool_call_id": "tc_1", "content": "{\"matches\": [\"plugin\"], \"total_deferred\": 5}", "is_error": false}]"#.to_string(),
        );
        let full_messages = vec![assistant, tool_result];
        assert!(!extract_discovered_deferred_tools(&full_messages, &deferred).is_empty());

        // Now simulate eviction — messages are gone
        let evicted_messages: Vec<ChatMessage> = vec![make_msg("user", "hello")];
        let result = extract_discovered_deferred_tools(&evicted_messages, &deferred);
        assert!(result.is_empty(), "tool unloads when its discovery message is evicted");
    }
}
