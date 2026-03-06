/// Parsed information from a hierarchical session key.
#[derive(Debug, Clone, Default)]
pub struct SessionKeyInfo {
    pub raw: String,
    pub channel: String,
    pub chat_type: String,
    pub chat_id: String,
    pub agent_id: String,
    pub is_subagent: bool,
    pub is_acp: bool,
    pub is_thread: bool,
    pub is_topic: bool,
    pub parent_key: String,
    pub rest: String,
}

/// Parse a hierarchical session key into components.
///
/// Key formats:
/// - `agent:<agentId>:rest` — Agent-scoped session
/// - `subagent:<parentId>:...` — Sub-agent session
/// - `acp:...` — ACP session
/// - `<channel>:group:<id>` — Group chat session
/// - `<channel>:channel:<id>` — Channel session
/// - `<channel>:dm:<id>` — Direct message session
/// - `<parent>:thread:<id>` — Threaded conversation
/// - `<parent>:topic:<id>` — Topic-grouped conversation
pub fn parse_session_key(key: &str) -> SessionKeyInfo {
    let mut info = SessionKeyInfo {
        raw: key.to_string(),
        ..Default::default()
    };

    if key.is_empty() {
        return info;
    }

    let parts: Vec<&str> = key.split(':').collect();
    if parts.is_empty() {
        return info;
    }

    // Check for special prefixes
    match parts[0] {
        "agent" => {
            if parts.len() >= 2 {
                info.agent_id = parts[1].to_string();
                if parts.len() > 2 {
                    info.rest = parts[2..].join(":");
                }
            }
            return info;
        }
        "subagent" => {
            info.is_subagent = true;
            if parts.len() > 1 {
                info.rest = parts[1..].join(":");
            }
            return info;
        }
        "acp" => {
            info.is_acp = true;
            if parts.len() > 1 {
                info.rest = parts[1..].join(":");
            }
            return info;
        }
        _ => {}
    }

    // Check for channel:type:id pattern
    if parts.len() >= 3 {
        info.channel = parts[0].to_string();

        match parts[1] {
            "group" => {
                info.chat_type = "group".to_string();
                info.chat_id = parts[2].to_string();
                if parts.len() > 3 {
                    info.rest = parts[3..].join(":");
                }
            }
            "channel" => {
                info.chat_type = "channel".to_string();
                info.chat_id = parts[2].to_string();
                if parts.len() > 3 {
                    info.rest = parts[3..].join(":");
                }
            }
            "dm" => {
                info.chat_type = "dm".to_string();
                info.chat_id = parts[2].to_string();
                if parts.len() > 3 {
                    info.rest = parts[3..].join(":");
                }
            }
            "thread" => {
                info.is_thread = true;
                info.chat_id = parts[2].to_string();
                info.parent_key = parts[0].to_string();
                if parts.len() > 3 {
                    info.rest = parts[3..].join(":");
                }
            }
            "topic" => {
                info.is_topic = true;
                info.chat_id = parts[2].to_string();
                info.parent_key = parts[0].to_string();
                if parts.len() > 3 {
                    info.rest = parts[3..].join(":");
                }
            }
            _ => {}
        }
    }

    // Check for thread/topic suffix in longer keys
    // Format: channel:type:id:thread:threadId
    // Start from index 2 to skip channel:type prefix
    for i in 0..parts.len().saturating_sub(1) {
        if parts[i] == "thread" {
            info.is_thread = true;
            info.chat_id = parts[i + 1].to_string();
            info.parent_key = parts[..i].join(":");
            if i + 2 < parts.len() {
                info.rest = parts[i + 2..].join(":");
            }
            break;
        }
        if parts[i] == "topic" {
            info.is_topic = true;
            info.chat_id = parts[i + 1].to_string();
            info.parent_key = parts[..i].join(":");
            if i + 2 < parts.len() {
                info.rest = parts[i + 2..].join(":");
            }
            break;
        }
    }

    info
}

/// Returns true if the key represents a subagent session.
pub fn is_subagent_key(key: &str) -> bool {
    key.starts_with("subagent:")
}

/// Returns true if the key represents an ACP session.
pub fn is_acp_key(key: &str) -> bool {
    key.starts_with("acp:")
}

/// Returns true if the key is agent-scoped.
pub fn is_agent_key(key: &str) -> bool {
    key.starts_with("agent:")
}

/// Extract the agent ID from an agent-scoped session key.
pub fn extract_agent_id(key: &str) -> String {
    parse_session_key(key).agent_id
}

/// Resolve the parent session key for a thread/topic session.
pub fn resolve_thread_parent_key(key: &str) -> String {
    let info = parse_session_key(key);
    if info.is_thread || info.is_topic {
        info.parent_key
    } else {
        String::new()
    }
}

/// Build a hierarchical session key from channel, chat type, and chat ID.
pub fn build_session_key(channel: &str, chat_type: &str, chat_id: &str) -> String {
    if channel.is_empty() || chat_type.is_empty() || chat_id.is_empty() {
        return String::new();
    }
    format!("{}:{}:{}", channel, chat_type, chat_id)
}

/// Build an agent-scoped session key.
pub fn build_agent_session_key(agent_id: &str, session_name: &str) -> String {
    if agent_id.is_empty() {
        return session_name.to_string();
    }
    if session_name.is_empty() {
        return format!("agent:{}", agent_id);
    }
    format!("agent:{}:{}", agent_id, session_name)
}

/// Build a subagent session key.
pub fn build_subagent_session_key(parent_id: &str, subagent_id: &str) -> String {
    if parent_id.is_empty() {
        return format!("subagent:{}", subagent_id);
    }
    format!("subagent:{}:{}", parent_id, subagent_id)
}

/// Build a thread session key from a parent key.
pub fn build_thread_session_key(parent_key: &str, thread_id: &str) -> String {
    format!("{}:thread:{}", parent_key, thread_id)
}

/// Build a topic session key from a parent key.
pub fn build_topic_session_key(parent_key: &str, topic_id: &str) -> String {
    format!("{}:topic:{}", parent_key, topic_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_agent_key() {
        let info = parse_session_key("agent:abc123:main");
        assert_eq!(info.agent_id, "abc123");
        assert_eq!(info.rest, "main");
        assert!(!info.is_subagent);
    }

    #[test]
    fn test_parse_subagent_key() {
        let info = parse_session_key("subagent:parent123:child456");
        assert!(info.is_subagent);
        assert_eq!(info.rest, "parent123:child456");
    }

    #[test]
    fn test_parse_acp_key() {
        let info = parse_session_key("acp:session1");
        assert!(info.is_acp);
        assert_eq!(info.rest, "session1");
    }

    #[test]
    fn test_parse_channel_group() {
        let info = parse_session_key("discord:group:12345");
        assert_eq!(info.channel, "discord");
        assert_eq!(info.chat_type, "group");
        assert_eq!(info.chat_id, "12345");
    }

    #[test]
    fn test_parse_channel_dm() {
        let info = parse_session_key("telegram:dm:user42");
        assert_eq!(info.channel, "telegram");
        assert_eq!(info.chat_type, "dm");
        assert_eq!(info.chat_id, "user42");
    }

    #[test]
    fn test_parse_thread() {
        let info = parse_session_key("discord:group:123:thread:t456");
        assert!(info.is_thread);
        assert_eq!(info.chat_id, "t456");
        assert_eq!(info.parent_key, "discord:group:123");
    }

    #[test]
    fn test_parse_topic() {
        let info = parse_session_key("slack:channel:abc:topic:t789");
        assert!(info.is_topic);
        assert_eq!(info.chat_id, "t789");
        assert_eq!(info.parent_key, "slack:channel:abc");
    }

    #[test]
    fn test_parse_empty() {
        let info = parse_session_key("");
        assert_eq!(info.raw, "");
        assert!(!info.is_subagent);
        assert!(!info.is_acp);
    }

    #[test]
    fn test_predicates() {
        assert!(is_subagent_key("subagent:x:y"));
        assert!(!is_subagent_key("agent:x"));
        assert!(is_acp_key("acp:session"));
        assert!(!is_acp_key("agent:x"));
        assert!(is_agent_key("agent:abc"));
        assert!(!is_agent_key("subagent:x"));
    }

    #[test]
    fn test_extract_agent_id() {
        assert_eq!(extract_agent_id("agent:mybot:rest"), "mybot");
        assert_eq!(extract_agent_id("subagent:x"), "");
    }

    #[test]
    fn test_build_session_key() {
        assert_eq!(
            build_session_key("discord", "group", "123"),
            "discord:group:123"
        );
        assert_eq!(build_session_key("", "group", "123"), "");
    }

    #[test]
    fn test_build_agent_session_key() {
        assert_eq!(
            build_agent_session_key("bot1", "main"),
            "agent:bot1:main"
        );
        assert_eq!(build_agent_session_key("bot1", ""), "agent:bot1");
        assert_eq!(build_agent_session_key("", "main"), "main");
    }

    #[test]
    fn test_build_subagent_session_key() {
        assert_eq!(
            build_subagent_session_key("parent", "child"),
            "subagent:parent:child"
        );
        assert_eq!(
            build_subagent_session_key("", "child"),
            "subagent:child"
        );
    }

    #[test]
    fn test_build_thread_session_key() {
        assert_eq!(
            build_thread_session_key("discord:group:123", "t1"),
            "discord:group:123:thread:t1"
        );
    }

    #[test]
    fn test_build_topic_session_key() {
        assert_eq!(
            build_topic_session_key("slack:channel:abc", "t2"),
            "slack:channel:abc:topic:t2"
        );
    }

    #[test]
    fn test_resolve_thread_parent_key() {
        assert_eq!(
            resolve_thread_parent_key("discord:group:123:thread:t456"),
            "discord:group:123"
        );
        assert_eq!(resolve_thread_parent_key("discord:group:123"), "");
    }
}
