package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"github.com/nebolabs/nebo/internal/agent/session"
)

// SessionsTool allows AI to query and manage sessions
type SessionsTool struct {
	sessions      *session.Manager
	currentUserID string
}

// SessionsInput defines the input for the sessions tool
type SessionsInput struct {
	Action     string `json:"action"`                // "list", "history", "status", "clear"
	SessionKey string `json:"session_key,omitempty"` // Session key (for history/status/clear)
	Limit      int    `json:"limit,omitempty"`       // Max messages to return (for history)
}

// NewSessionsTool creates a new sessions tool
func NewSessionsTool(sessions *session.Manager) *SessionsTool {
	return &SessionsTool{
		sessions: sessions,
	}
}

// SetCurrentUser sets the user ID for user-scoped session operations
// This should be called before each request to ensure proper isolation
func (t *SessionsTool) SetCurrentUser(userID string) {
	t.currentUserID = userID
}

// GetCurrentUser returns the current user ID
func (t *SessionsTool) GetCurrentUser() string {
	return t.currentUserID
}

// Name returns the tool name
func (t *SessionsTool) Name() string {
	return "sessions"
}

// Description returns the tool description
func (t *SessionsTool) Description() string {
	return "Query and manage conversation sessions. List all sessions, view history, check status, or clear a session."
}

// Schema returns the JSON schema
func (t *SessionsTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"description": "Action to perform: 'list' (all sessions), 'history' (view messages), 'status' (session info), 'clear' (reset session)",
				"enum": ["list", "history", "status", "clear"]
			},
			"session_key": {
				"type": "string",
				"description": "Session key (required for history/status/clear). Use 'list' action first to see available keys."
			},
			"limit": {
				"type": "integer",
				"description": "Maximum messages to return for 'history' action (default: 20)",
				"default": 20
			}
		},
		"required": ["action"]
	}`)
}

// RequiresApproval returns false for read operations, true for clear
func (t *SessionsTool) RequiresApproval() bool {
	return false // We handle clear specially inside Execute
}

// Execute performs the session operation
func (t *SessionsTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	if t.sessions == nil {
		return &ToolResult{
			Content: "Error: Session manager not configured",
			IsError: true,
		}, nil
	}

	var params SessionsInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Invalid input: %v", err),
			IsError: true,
		}, nil
	}

	switch params.Action {
	case "list":
		return t.listSessions()
	case "history":
		if params.SessionKey == "" {
			return &ToolResult{
				Content: "Error: 'session_key' is required for history action",
				IsError: true,
			}, nil
		}
		return t.sessionHistory(params.SessionKey, params.Limit)
	case "status":
		if params.SessionKey == "" {
			return &ToolResult{
				Content: "Error: 'session_key' is required for status action",
				IsError: true,
			}, nil
		}
		return t.sessionStatus(params.SessionKey)
	case "clear":
		if params.SessionKey == "" {
			return &ToolResult{
				Content: "Error: 'session_key' is required for clear action",
				IsError: true,
			}, nil
		}
		return t.clearSession(params.SessionKey)
	default:
		return &ToolResult{
			Content: fmt.Sprintf("Unknown action: %s. Use 'list', 'history', 'status', or 'clear'", params.Action),
			IsError: true,
		}, nil
	}
}

// listSessions returns all sessions for the current user
func (t *SessionsTool) listSessions() (*ToolResult, error) {
	sessions, err := t.sessions.ListSessions(t.currentUserID)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error listing sessions: %v", err),
			IsError: true,
		}, nil
	}

	if len(sessions) == 0 {
		return &ToolResult{
			Content: "No sessions found.",
			IsError: false,
		}, nil
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Sessions (%d total):\n\n", len(sessions)))
	sb.WriteString("| Session Key | Created | Updated |\n")
	sb.WriteString("|------------|---------|----------|\n")

	for _, sess := range sessions {
		created := sess.CreatedAt.Format("2006-01-02 15:04")
		updated := sess.UpdatedAt.Format("2006-01-02 15:04")
		sb.WriteString(fmt.Sprintf("| %s | %s | %s |\n", sess.SessionKey, created, updated))
	}

	return &ToolResult{
		Content: sb.String(),
		IsError: false,
	}, nil
}

// sessionHistory returns messages from a session
func (t *SessionsTool) sessionHistory(sessionKey string, limit int) (*ToolResult, error) {
	if limit <= 0 {
		limit = 20
	}
	if limit > 100 {
		limit = 100
	}

	sess, err := t.sessions.GetOrCreate(sessionKey, t.currentUserID)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error getting session: %v", err),
			IsError: true,
		}, nil
	}

	messages, err := t.sessions.GetMessages(sess.ID, limit)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error getting messages: %v", err),
			IsError: true,
		}, nil
	}

	if len(messages) == 0 {
		return &ToolResult{
			Content: fmt.Sprintf("No messages in session: %s", sessionKey),
			IsError: false,
		}, nil
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Session: %s (showing %d messages)\n", sessionKey, len(messages)))
	sb.WriteString(strings.Repeat("=", 60) + "\n\n")

	for i, msg := range messages {
		roleIcon := "👤"
		switch msg.Role {
		case "assistant":
			roleIcon = "🤖"
		case "system":
			roleIcon = "⚙️"
		case "tool":
			roleIcon = "🔧"
		}

		sb.WriteString(fmt.Sprintf("%d. %s [%s]\n", i+1, roleIcon, msg.Role))

		if msg.Content != "" {
			content := msg.Content
			if len(content) > 500 {
				content = content[:497] + "..."
			}
			sb.WriteString(fmt.Sprintf("   %s\n", strings.ReplaceAll(content, "\n", "\n   ")))
		}

		if len(msg.ToolCalls) > 0 {
			var calls []session.ToolCall
			json.Unmarshal(msg.ToolCalls, &calls)
			for _, tc := range calls {
				sb.WriteString(fmt.Sprintf("   → Tool: %s\n", tc.Name))
			}
		}

		if len(msg.ToolResults) > 0 {
			var results []session.ToolResult
			json.Unmarshal(msg.ToolResults, &results)
			for _, tr := range results {
				status := "✓"
				if tr.IsError {
					status = "✗"
				}
				content := tr.Content
				if len(content) > 200 {
					content = content[:197] + "..."
				}
				sb.WriteString(fmt.Sprintf("   %s Result: %s\n", status, content))
			}
		}

		sb.WriteString("\n")
	}

	return &ToolResult{
		Content: sb.String(),
		IsError: false,
	}, nil
}

// sessionStatus returns status info about a session
func (t *SessionsTool) sessionStatus(sessionKey string) (*ToolResult, error) {
	sess, err := t.sessions.GetOrCreate(sessionKey, t.currentUserID)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error getting session: %v", err),
			IsError: true,
		}, nil
	}

	messages, err := t.sessions.GetMessages(sess.ID, 1000) // Get all to count
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error getting messages: %v", err),
			IsError: true,
		}, nil
	}

	// Count by role
	userCount := 0
	assistantCount := 0
	toolCount := 0
	totalToolCalls := 0

	for _, msg := range messages {
		switch msg.Role {
		case "user":
			userCount++
		case "assistant":
			assistantCount++
			if len(msg.ToolCalls) > 0 {
				var calls []session.ToolCall
				json.Unmarshal(msg.ToolCalls, &calls)
				totalToolCalls += len(calls)
			}
		case "tool":
			toolCount++
		}
	}

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("Session Status: %s\n", sessionKey))
	sb.WriteString(strings.Repeat("=", 40) + "\n\n")
	sb.WriteString(fmt.Sprintf("ID: %s\n", sess.ID))
	sb.WriteString(fmt.Sprintf("Created: %s\n", sess.CreatedAt.Format(time.RFC3339)))
	sb.WriteString(fmt.Sprintf("Updated: %s\n", sess.UpdatedAt.Format(time.RFC3339)))
	sb.WriteString(fmt.Sprintf("Age: %s\n", time.Since(sess.CreatedAt).Round(time.Minute)))
	sb.WriteString("\n")
	sb.WriteString(fmt.Sprintf("Total Messages: %d\n", len(messages)))
	sb.WriteString(fmt.Sprintf("  User: %d\n", userCount))
	sb.WriteString(fmt.Sprintf("  Assistant: %d\n", assistantCount))
	sb.WriteString(fmt.Sprintf("  Tool Results: %d\n", toolCount))
	sb.WriteString(fmt.Sprintf("Total Tool Calls: %d\n", totalToolCalls))

	return &ToolResult{
		Content: sb.String(),
		IsError: false,
	}, nil
}

// clearSession resets a session
func (t *SessionsTool) clearSession(sessionKey string) (*ToolResult, error) {
	sess, err := t.sessions.GetOrCreate(sessionKey, t.currentUserID)
	if err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error getting session: %v", err),
			IsError: true,
		}, nil
	}

	if err := t.sessions.Reset(sess.ID); err != nil {
		return &ToolResult{
			Content: fmt.Sprintf("Error clearing session: %v", err),
			IsError: true,
		}, nil
	}

	return &ToolResult{
		Content: fmt.Sprintf("Session cleared: %s", sessionKey),
		IsError: false,
	}, nil
}
