package session

import "strings"

// SessionKeyInfo contains parsed information from a session key
type SessionKeyInfo struct {
	Raw       string // Original key
	Channel   string // Channel type: discord, telegram, slack, etc.
	ChatType  string // Chat type: group, channel, dm
	ChatID    string // Chat identifier
	AgentID   string // Agent ID if agent-scoped
	IsSubagent bool   // True if this is a subagent session
	IsACP      bool   // True if this is an ACP session
	IsThread   bool   // True if this is a threaded conversation
	IsTopic    bool   // True if this is a topic-grouped conversation
	ParentKey  string // Parent session key for threads/topics
	Rest       string // Remaining key parts
}

// ParseSessionKey parses a hierarchical session key into components.
// Key formats:
//   - "agent:<agentId>:rest"           - Agent-scoped session
//   - "subagent:<parentId>:..."        - Sub-agent session
//   - "acp:..."                        - ACP session
//   - "<channel>:group:<id>"           - Group chat session
//   - "<channel>:channel:<id>"         - Channel session
//   - "<channel>:dm:<id>"              - Direct message session
//   - "<parent>:thread:<id>"           - Threaded conversation
//   - "<parent>:topic:<id>"            - Topic-grouped conversation
func ParseSessionKey(key string) *SessionKeyInfo {
	info := &SessionKeyInfo{Raw: key}

	if key == "" {
		return info
	}

	parts := strings.Split(key, ":")
	if len(parts) == 0 {
		return info
	}

	// Check for special prefixes
	switch parts[0] {
	case "agent":
		if len(parts) >= 2 {
			info.AgentID = parts[1]
			if len(parts) > 2 {
				info.Rest = strings.Join(parts[2:], ":")
			}
		}
		return info

	case "subagent":
		info.IsSubagent = true
		if len(parts) > 1 {
			info.Rest = strings.Join(parts[1:], ":")
		}
		return info

	case "acp":
		info.IsACP = true
		if len(parts) > 1 {
			info.Rest = strings.Join(parts[1:], ":")
		}
		return info
	}

	// Check for channel:type:id pattern
	if len(parts) >= 3 {
		info.Channel = parts[0]

		switch parts[1] {
		case "group":
			info.ChatType = "group"
			info.ChatID = parts[2]
			if len(parts) > 3 {
				info.Rest = strings.Join(parts[3:], ":")
			}

		case "channel":
			info.ChatType = "channel"
			info.ChatID = parts[2]
			if len(parts) > 3 {
				info.Rest = strings.Join(parts[3:], ":")
			}

		case "dm":
			info.ChatType = "dm"
			info.ChatID = parts[2]
			if len(parts) > 3 {
				info.Rest = strings.Join(parts[3:], ":")
			}

		case "thread":
			info.IsThread = true
			info.ChatID = parts[2]
			// Parent is the prefix before :thread:
			info.ParentKey = parts[0]
			if len(parts) > 3 {
				info.Rest = strings.Join(parts[3:], ":")
			}

		case "topic":
			info.IsTopic = true
			info.ChatID = parts[2]
			// Parent is the prefix before :topic:
			info.ParentKey = parts[0]
			if len(parts) > 3 {
				info.Rest = strings.Join(parts[3:], ":")
			}
		}
	}

	// Check for thread/topic suffix in longer keys
	// Format: channel:type:id:thread:threadId
	for i := 0; i < len(parts)-2; i++ {
		if parts[i] == "thread" {
			info.IsThread = true
			info.ChatID = parts[i+1]
			info.ParentKey = strings.Join(parts[:i], ":")
			if i+2 < len(parts) {
				info.Rest = strings.Join(parts[i+2:], ":")
			}
			break
		}
		if parts[i] == "topic" {
			info.IsTopic = true
			info.ChatID = parts[i+1]
			info.ParentKey = strings.Join(parts[:i], ":")
			if i+2 < len(parts) {
				info.Rest = strings.Join(parts[i+2:], ":")
			}
			break
		}
	}

	return info
}

// ResolveThreadParentKey returns the parent session key for a thread/topic session
func ResolveThreadParentKey(key string) string {
	info := ParseSessionKey(key)
	if info.IsThread || info.IsTopic {
		return info.ParentKey
	}
	return ""
}

// IsSubagentKey returns true if the key represents a subagent session
func IsSubagentKey(key string) bool {
	return strings.HasPrefix(key, "subagent:")
}

// IsACPKey returns true if the key represents an ACP session
func IsACPKey(key string) bool {
	return strings.HasPrefix(key, "acp:")
}

// IsAgentKey returns true if the key is agent-scoped
func IsAgentKey(key string) bool {
	return strings.HasPrefix(key, "agent:")
}

// ExtractAgentID extracts the agent ID from an agent-scoped session key
func ExtractAgentID(key string) string {
	info := ParseSessionKey(key)
	return info.AgentID
}

// BuildSessionKey builds a hierarchical session key
func BuildSessionKey(channel, chatType, chatID string) string {
	if channel == "" || chatType == "" || chatID == "" {
		return ""
	}
	return channel + ":" + chatType + ":" + chatID
}

// BuildAgentSessionKey builds an agent-scoped session key
func BuildAgentSessionKey(agentID, sessionName string) string {
	if agentID == "" {
		return sessionName
	}
	if sessionName == "" {
		return "agent:" + agentID
	}
	return "agent:" + agentID + ":" + sessionName
}

// BuildSubagentSessionKey builds a subagent session key
func BuildSubagentSessionKey(parentID, subagentID string) string {
	if parentID == "" {
		return "subagent:" + subagentID
	}
	return "subagent:" + parentID + ":" + subagentID
}

// BuildThreadSessionKey builds a thread session key from a parent key
func BuildThreadSessionKey(parentKey, threadID string) string {
	return parentKey + ":thread:" + threadID
}

// BuildTopicSessionKey builds a topic session key from a parent key
func BuildTopicSessionKey(parentKey, topicID string) string {
	return parentKey + ":topic:" + topicID
}
