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
/// - `agent:<agentId>:<channel>` — Agent-scoped session
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
                    info.channel = parts[2].to_string();
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

/// Extract the agent ID from an agent-scoped session key — the ONE extractor
/// (CODE_AUDITOR Rule 8; audit 2026-08-22 found 12 inline `split(':')` copies
/// that returned the literal "agent" on subagent keys).
///
/// Subagent runs nest the parent's FULL key (`subagent:<parent_key>:<task_id>`)
/// and the parent is itself `agent:<id>:…` — a naive split silently scopes
/// per-agent state (plugin account profiles, learned skills, event
/// attribution) to a bogus entity, falling back to global credentials. Strip
/// any number of `subagent:` wrappers first. Returns "" for non-agent keys.
pub fn extract_agent_id(key: &str) -> String {
    let mut inner = key;
    while let Some(rest) = inner.strip_prefix("subagent:") {
        inner = rest;
    }
    parse_session_key(inner).agent_id
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

/// Build an agent-scoped session key: `agent:<agentId>:<channel>`.
pub fn build_agent_session_key(agent_id: &str, channel: &str) -> String {
    if agent_id.is_empty() {
        return channel.to_string();
    }
    if channel.is_empty() {
        return format!("agent:{}", agent_id);
    }
    format!("agent:{}:{}", agent_id, channel)
}

/// Prefix that matches every session belonging to an agent (`agent:<id>:`).
/// The ONE builder for prefix checks — hand-built copies had drifted into
/// authorization checks and list queries (audit 2026-08-22).
pub fn agent_session_prefix(agent_id: &str) -> String {
    format!("agent:{}:", agent_id)
}

/// The workflow-id namespace for an agent's inline workflow bindings.
/// DELIBERATELY a separate helper even though the literal shape (`agent:<id>`)
/// collides with a channel-less agent session key: a workflow id is NOT a
/// session key — never feed one to the session-key parsers.
pub fn agent_workflow_id(agent_id: &str) -> String {
    format!("agent:{}", agent_id)
}

/// Inverse of [`agent_workflow_id`]. `None` when the id is not agent-scoped.
pub fn agent_id_from_workflow_id(workflow_id: &str) -> Option<&str> {
    workflow_id.strip_prefix("agent:").filter(|s| !s.is_empty())
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

/// The chat id embedded in a thread-scoped session key: everything after the
/// FIRST `:thread:` marker, verbatim. `None` when the key has no `:thread:`.
///
/// This is the ONE home for the raw `find(":thread:")` slicing convention
/// (previously inline in ws.rs dispatch_chat): `agent:<id>:thread:<chat>`
/// keys parse with channel == "thread" and is_thread == FALSE (the agent
/// prefix returns early from `parse_session_key`), so the chat id CANNOT be
/// read via `SessionKeyInfo` — see the tripwire in chat_dispatch.rs's
/// session_key_contract_tests. The remainder is NOT re-parsed: a key with
/// trailing segments after the chat id (e.g. a subagent-wrapped parent key)
/// returns them attached, exactly as the call-site slicing always did.
pub fn chat_id_from_thread_key(key: &str) -> Option<&str> {
    key.find(":thread:").map(|pos| &key[pos + ":thread:".len()..])
}

/// Build a topic session key from a parent key.
pub fn build_topic_session_key(parent_key: &str, topic_id: &str) -> String {
    format!("{}:topic:{}", parent_key, topic_id)
}

/// The agent id embedded in an a2ui surface id (`agent:{agentId}:{view}`):
/// the second `:`-segment, verbatim. `None` when the id has fewer than two
/// segments.
///
/// Mirrors the ws.rs a2ui_action call site exactly: the leading literal is
/// NOT validated (any `x:y…` shape yields `Some("y")`), and an empty second
/// segment yields `Some("")` — callers treat empty as missing. A surface id
/// is NOT a session key; never feed one to the session-key parsers.
pub fn agent_id_from_surface_id(surface_id: &str) -> Option<&str> {
    let mut parts = surface_id.split(':');
    parts.next()?;
    parts.next()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Subagent keys must resolve to the PARENT's agent id, however deeply
    /// nested — the naive split returned "agent" and misattributed state.
    #[test]
    fn extract_agent_id_pierces_subagent_wrappers() {
        assert_eq!(extract_agent_id("agent:abc-123:web"), "abc-123");
        assert_eq!(
            extract_agent_id("subagent:agent:abc-123:thread:t1:task-9"),
            "abc-123"
        );
        assert_eq!(
            extract_agent_id("subagent:subagent:agent:abc-123:web:sa-1:sa-2"),
            "abc-123"
        );
        assert_eq!(extract_agent_id("agent:cos-uuid:workflow:run-42"), "cos-uuid");
        assert_eq!(extract_agent_id("main"), "");
        assert_eq!(extract_agent_id("acp:xyz"), "");
    }

    #[test]
    fn test_parse_agent_key() {
        let info = parse_session_key("agent:abc123:web");
        assert_eq!(info.agent_id, "abc123");
        assert_eq!(info.channel, "web");
        assert_eq!(info.rest, "web");
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
        assert_eq!(build_agent_session_key("bot1", "main"), "agent:bot1:main");
        assert_eq!(build_agent_session_key("bot1", ""), "agent:bot1");
        assert_eq!(build_agent_session_key("", "main"), "main");
    }

    #[test]
    fn test_build_subagent_session_key() {
        assert_eq!(
            build_subagent_session_key("parent", "child"),
            "subagent:parent:child"
        );
        assert_eq!(build_subagent_session_key("", "child"), "subagent:child");
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

    /// Surface ids (`agent:{id}:{view}`) yield the second segment; fewer
    /// than two segments yield None. The prefix is deliberately unvalidated
    /// and an empty second segment comes back as Some("") — both locked to
    /// match the ws.rs a2ui_action semantics this replaced.
    #[test]
    fn agent_id_from_surface_id_takes_the_second_segment() {
        assert_eq!(agent_id_from_surface_id("agent:abc:view"), Some("abc"));
        assert_eq!(
            agent_id_from_surface_id("agent:abc:view:extra"),
            Some("abc")
        );
        assert_eq!(agent_id_from_surface_id("foo:bar"), Some("bar"));
        assert_eq!(agent_id_from_surface_id("agent::view"), Some(""));
        assert_eq!(agent_id_from_surface_id("agent"), None);
        assert_eq!(agent_id_from_surface_id(""), None);
    }

    /// Thread keys yield the chat id after the FIRST `:thread:` marker;
    /// non-thread keys yield None. This backs ws.rs's turn_chat_id
    /// resolution, where SessionKeyInfo.is_thread is useless for
    /// agent-prefixed keys.
    #[test]
    fn chat_id_from_thread_key_slices_after_first_marker() {
        assert_eq!(
            chat_id_from_thread_key("agent:a1:thread:chat-42"),
            Some("chat-42")
        );
        assert_eq!(
            chat_id_from_thread_key("discord:group:123:thread:t456"),
            Some("t456")
        );
        assert_eq!(chat_id_from_thread_key("agent:a1:web"), None);
        assert_eq!(chat_id_from_thread_key(""), None);
    }

    /// The remainder is verbatim, not re-parsed: a subagent-wrapped thread
    /// key keeps its trailing task segment attached, and a second `:thread:`
    /// marker is NOT a new split point — first marker wins. Locks the exact
    /// semantics of the original ws.rs slicing.
    #[test]
    fn chat_id_from_thread_key_keeps_the_raw_remainder() {
        assert_eq!(
            chat_id_from_thread_key("subagent:agent:a1:thread:chat-42:sa-1"),
            Some("chat-42:sa-1")
        );
        assert_eq!(
            chat_id_from_thread_key("x:thread:a:thread:b"),
            Some("a:thread:b")
        );
        assert_eq!(chat_id_from_thread_key("agent:a1:thread:"), Some(""));
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
