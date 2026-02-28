package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"
)

// SessionQuerier provides read-only access to sessions and messages.
// Implemented by session.Manager — defined here to avoid import cycles.
type SessionQuerier interface {
	ListSessions(userID string) ([]SessionInfo, error)
	GetMessages(sessionID string, limit int) ([]SessionMessage, error)
	GetOrCreate(sessionKey, userID string) (*SessionInfo, error)
}

// SessionInfo is a minimal session descriptor for the query tool.
type SessionInfo struct {
	ID         string
	SessionKey string
	CreatedAt  time.Time
	UpdatedAt  time.Time
}

// SessionMessage is a minimal message descriptor for the query tool.
type SessionMessage struct {
	Role    string
	Content string
}

// QuerySessionsTool allows Main Chat to read messages from other sessions.
// This gives the owner a command-center view of what's happening across
// loop channels, bot DMs, and background work.
type QuerySessionsTool struct {
	querier SessionQuerier
}

type querySessionsInput struct {
	Action     string `json:"action"`               // "list" or "read"
	SessionKey string `json:"session_key,omitempty"` // Session to read
	Limit      int    `json:"limit,omitempty"`       // Max messages (default 10)
}

func (t *QuerySessionsTool) Name() string        { return "query_sessions" }
func (t *QuerySessionsTool) RequiresApproval() bool { return false }

func (t *QuerySessionsTool) Description() string {
	return "Read messages from other sessions. Use 'list' to see all sessions, 'read' to get messages from a specific session. Loop channel sessions have keys like 'loop-channel-{channelID}-{convID}'."
}

func (t *QuerySessionsTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"description": "Action: 'list' (list sessions) or 'read' (read messages from a session)",
				"enum": ["list", "read"]
			},
			"session_key": {
				"type": "string",
				"description": "Session key to read messages from (required for 'read' action)"
			},
			"limit": {
				"type": "integer",
				"description": "Maximum number of messages to return (default 10, max 50)"
			}
		},
		"required": ["action"]
	}`)
}

func (t *QuerySessionsTool) Execute(_ context.Context, input json.RawMessage) (*ToolResult, error) {
	var in querySessionsInput
	if err := json.Unmarshal(input, &in); err != nil {
		return &ToolResult{Content: "Invalid input: " + err.Error(), IsError: true}, nil
	}

	if t.querier == nil {
		return &ToolResult{Content: "Session querier not configured", IsError: true}, nil
	}

	switch in.Action {
	case "list":
		return t.listSessions()
	case "read":
		if in.SessionKey == "" {
			return &ToolResult{Content: "session_key is required for 'read' action", IsError: true}, nil
		}
		limit := in.Limit
		if limit <= 0 {
			limit = 10
		}
		if limit > 50 {
			limit = 50
		}
		return t.readSession(in.SessionKey, limit)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", in.Action), IsError: true}, nil
	}
}

func (t *QuerySessionsTool) listSessions() (*ToolResult, error) {
	sessions, err := t.querier.ListSessions("")
	if err != nil {
		return &ToolResult{Content: "Failed to list sessions: " + err.Error(), IsError: true}, nil
	}

	if len(sessions) == 0 {
		return &ToolResult{Content: "No sessions found"}, nil
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Sessions (%d):\n", len(sessions)))
	for _, s := range sessions {
		sb.WriteString(fmt.Sprintf("- %s (updated: %s)\n", s.SessionKey, s.UpdatedAt.Format("2006-01-02 15:04")))
	}
	return &ToolResult{Content: sb.String()}, nil
}

func (t *QuerySessionsTool) readSession(sessionKey string, limit int) (*ToolResult, error) {
	// Resolve session key to session ID
	sess, err := t.querier.GetOrCreate(sessionKey, "")
	if err != nil {
		return &ToolResult{Content: "Session not found: " + err.Error(), IsError: true}, nil
	}

	messages, err := t.querier.GetMessages(sess.ID, limit)
	if err != nil {
		return &ToolResult{Content: "Failed to read messages: " + err.Error(), IsError: true}, nil
	}

	if len(messages) == 0 {
		return &ToolResult{Content: fmt.Sprintf("No messages in session '%s'", sessionKey)}, nil
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Messages from '%s' (last %d):\n\n", sessionKey, len(messages)))
	for _, m := range messages {
		content := m.Content
		if len(content) > 500 {
			content = content[:497] + "..."
		}
		sb.WriteString(fmt.Sprintf("[%s] %s\n\n", m.Role, content))
	}
	return &ToolResult{Content: sb.String()}, nil
}

// SetQuerier sets the session querier (for late binding).
func (t *QuerySessionsTool) SetQuerier(q SessionQuerier) {
	t.querier = q
}
