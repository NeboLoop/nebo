package db

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"time"

	"github.com/google/uuid"
)

// AgentMessage represents a conversation message for the agent (wrapper around sqlc SessionMessage)
type AgentMessage struct {
	ID          int64           `json:"id,omitempty"`
	SessionID   string          `json:"session_id"`
	Role        string          `json:"role"` // user, assistant, system, tool
	Content     string          `json:"content,omitempty"`
	ToolCalls   json.RawMessage `json:"tool_calls,omitempty"`
	ToolResults json.RawMessage `json:"tool_results,omitempty"`
	CreatedAt   time.Time       `json:"created_at"`
}

// AgentToolCall represents a tool invocation
type AgentToolCall struct {
	ID    string          `json:"id"`
	Name  string          `json:"name"`
	Input json.RawMessage `json:"input"`
}

// AgentToolResult represents the result of a tool execution
type AgentToolResult struct {
	ToolCallID string `json:"tool_call_id"`
	Content    string `json:"content"`
	IsError    bool   `json:"is_error,omitempty"`
}

// AgentSession represents a conversation session for the agent
type AgentSession struct {
	ID         string    `json:"id"`
	SessionKey string    `json:"session_key"` // Maps to 'name' column in DB
	CreatedAt  time.Time `json:"created_at"`
	UpdatedAt  time.Time `json:"updated_at"`
}

// SessionManager handles session persistence using sqlc-generated queries
type SessionManager struct {
	queries *Queries
	rawDB   *sql.DB
}

// NewSessionManager creates a session manager from a Store
func NewSessionManager(store *Store) *SessionManager {
	return &SessionManager{
		queries: store.Queries,
		rawDB:   store.db,
	}
}

// NewSessionManagerFromDB creates a session manager from a raw database connection
func NewSessionManagerFromDB(sqlDB *sql.DB) *SessionManager {
	return &SessionManager{
		queries: New(sqlDB),
		rawDB:   sqlDB,
	}
}

// GetDB returns the underlying database connection for sharing with other components
func (m *SessionManager) GetDB() *sql.DB {
	return m.rawDB
}

// GetOrCreate returns an existing session or creates a new one
// If userID is provided, session is scoped to that user; otherwise uses agent scope
func (m *SessionManager) GetOrCreate(sessionKey, userID string) (*AgentSession, error) {
	ctx := context.Background()
	scope := "agent"
	scopeID := ""
	if userID != "" {
		scope = "user"
		scopeID = userID
	}

	// Try to get existing session
	var dbSession Session
	var err error

	if scopeID == "" {
		dbSession, err = m.queries.GetSessionByNameAndScopeNullID(ctx, GetSessionByNameAndScopeNullIDParams{
			Name:  sql.NullString{String: sessionKey, Valid: true},
			Scope: sql.NullString{String: scope, Valid: true},
		})
	} else {
		dbSession, err = m.queries.GetSessionByNameAndScope(ctx, GetSessionByNameAndScopeParams{
			Name:    sql.NullString{String: sessionKey, Valid: true},
			Scope:   sql.NullString{String: scope, Valid: true},
			ScopeID: sql.NullString{String: scopeID, Valid: true},
		})
	}

	if err == nil {
		return &AgentSession{
			ID:         dbSession.ID,
			SessionKey: dbSession.Name.String,
			CreatedAt:  time.Unix(dbSession.CreatedAt, 0),
			UpdatedAt:  time.Unix(dbSession.UpdatedAt, 0),
		}, nil
	}
	if err != sql.ErrNoRows {
		return nil, err
	}

	// Create new session
	id := uuid.New().String()
	dbSession, err = m.queries.CreateSession(ctx, CreateSessionParams{
		ID:       id,
		Name:     sql.NullString{String: sessionKey, Valid: true},
		Scope:    sql.NullString{String: scope, Valid: true},
		ScopeID:  sql.NullString{String: scopeID, Valid: scopeID != ""},
		Metadata: sql.NullString{},
	})
	if err != nil {
		return nil, fmt.Errorf("failed to create session: %w", err)
	}

	return &AgentSession{
		ID:         dbSession.ID,
		SessionKey: dbSession.Name.String,
		CreatedAt:  time.Unix(dbSession.CreatedAt, 0),
		UpdatedAt:  time.Unix(dbSession.UpdatedAt, 0),
	}, nil
}

// GetSummary retrieves the compaction summary for a session (if any)
func (m *SessionManager) GetSummary(sessionID string) (string, error) {
	ctx := context.Background()
	dbSession, err := m.queries.GetSession(ctx, sessionID)
	if err != nil {
		return "", err
	}
	if dbSession.Summary.Valid {
		return dbSession.Summary.String, nil
	}
	return "", nil
}

// GetMessages retrieves messages for a session with an optional limit
// Orders by id (auto-increment) to preserve insertion order
func (m *SessionManager) GetMessages(sessionID string, limit int) ([]AgentMessage, error) {
	ctx := context.Background()

	var dbMessages []SessionMessage
	var err error

	if limit > 0 {
		dbMessages, err = m.queries.GetRecentNonCompactedMessages(ctx, GetRecentNonCompactedMessagesParams{
			SessionID: sessionID,
			Limit:     int64(limit),
		})
	} else {
		dbMessages, err = m.queries.GetNonCompactedMessages(ctx, sessionID)
	}
	if err != nil {
		return nil, err
	}

	messages := make([]AgentMessage, 0, len(dbMessages))
	for _, dbMsg := range dbMessages {
		msg := AgentMessage{
			ID:        dbMsg.ID,
			SessionID: dbMsg.SessionID,
			Role:      dbMsg.Role,
			Content:   dbMsg.Content.String,
			CreatedAt: time.Unix(dbMsg.CreatedAt, 0),
		}
		if dbMsg.ToolCalls.Valid && dbMsg.ToolCalls.String != "" {
			msg.ToolCalls = json.RawMessage(dbMsg.ToolCalls.String)
		}
		if dbMsg.ToolResults.Valid && dbMsg.ToolResults.String != "" {
			msg.ToolResults = json.RawMessage(dbMsg.ToolResults.String)
		}
		messages = append(messages, msg)
	}

	// Sanitize messages: strip orphaned tool_results that have no matching tool_calls
	return sanitizeAgentMessages(messages), nil
}

// AppendMessage adds a message to a session
func (m *SessionManager) AppendMessage(sessionID string, msg AgentMessage) error {
	ctx := context.Background()

	var toolCalls, toolResults sql.NullString
	if len(msg.ToolCalls) > 0 {
		toolCalls = sql.NullString{String: string(msg.ToolCalls), Valid: true}
	}
	if len(msg.ToolResults) > 0 {
		toolResults = sql.NullString{String: string(msg.ToolResults), Valid: true}
	}

	_, err := m.queries.CreateSessionMessage(ctx, CreateSessionMessageParams{
		SessionID:     sessionID,
		Role:          msg.Role,
		Content:       sql.NullString{String: msg.Content, Valid: msg.Content != ""},
		ToolCalls:     toolCalls,
		ToolResults:   toolResults,
		TokenEstimate: sql.NullInt64{},
	})
	if err != nil {
		return fmt.Errorf("failed to append message: %w", err)
	}

	// Update session message count
	return m.queries.IncrementSessionMessageCount(ctx, sessionID)
}

// Compact summarizes old messages to reduce context size
func (m *SessionManager) Compact(sessionID string, summaryContent string) error {
	ctx := context.Background()

	// Get message count
	count, err := m.queries.CountSessionMessages(ctx, sessionID)
	if err != nil {
		return err
	}

	// Keep the last 10 messages, summarize the rest
	keepCount := int64(10)
	if count <= keepCount {
		return nil // Nothing to compact
	}

	// Get the min ID of messages to keep
	minIDResult, err := m.queries.GetMaxMessageIDToKeep(ctx, GetMaxMessageIDToKeepParams{
		SessionID: sessionID,
		Limit:     keepCount,
	})
	if err != nil {
		return err
	}

	// Handle type assertion for COALESCE result
	var minIDToKeep int64
	switch v := minIDResult.(type) {
	case int64:
		minIDToKeep = v
	case int:
		minIDToKeep = int64(v)
	case float64:
		minIDToKeep = int64(v)
	}

	// Mark messages before that ID as compacted
	if minIDToKeep > 0 {
		err = m.queries.MarkMessagesCompactedBeforeID(ctx, MarkMessagesCompactedBeforeIDParams{
			SessionID: sessionID,
			ID:        minIDToKeep,
		})
		if err != nil {
			return err
		}
	}

	// Update session with summary and increment compaction count
	return m.queries.CompactSession(ctx, CompactSessionParams{
		Summary: sql.NullString{String: summaryContent, Valid: true},
		ID:      sessionID,
	})
}

// GetCompactionCount returns the number of times this session has been compacted
func (m *SessionManager) GetCompactionCount(sessionID string) (int, error) {
	ctx := context.Background()
	dbSession, err := m.queries.GetSession(ctx, sessionID)
	if err != nil {
		return 0, err
	}
	if dbSession.CompactionCount.Valid {
		return int(dbSession.CompactionCount.Int64), nil
	}
	return 0, nil
}

// GetMemoryFlushCompactionCount returns the compaction count at which memory flush last ran
func (m *SessionManager) GetMemoryFlushCompactionCount(sessionID string) (int, error) {
	ctx := context.Background()
	dbSession, err := m.queries.GetSession(ctx, sessionID)
	if err != nil {
		return -1, err
	}
	if dbSession.MemoryFlushCompactionCount.Valid {
		return int(dbSession.MemoryFlushCompactionCount.Int64), nil
	}
	return -1, nil // Never flushed
}

// RecordMemoryFlush marks that memory flush ran at the current compaction cycle
func (m *SessionManager) RecordMemoryFlush(sessionID string) error {
	ctx := context.Background()
	return m.queries.RecordMemoryFlush(ctx, sessionID)
}

// ShouldRunMemoryFlush checks if memory flush should run based on compaction cycle
func (m *SessionManager) ShouldRunMemoryFlush(sessionID string) (bool, error) {
	ctx := context.Background()
	dbSession, err := m.queries.GetSession(ctx, sessionID)
	if err != nil {
		return true, err // Default to true on error
	}

	// If no compaction has happened yet, allow flush
	if !dbSession.CompactionCount.Valid || dbSession.CompactionCount.Int64 == 0 {
		return true, nil
	}

	// If never flushed, should flush
	if !dbSession.MemoryFlushCompactionCount.Valid {
		return true, nil
	}

	// Only flush if compaction count has changed since last flush
	return dbSession.CompactionCount.Int64 != dbSession.MemoryFlushCompactionCount.Int64, nil
}

// Reset clears all messages from a session
func (m *SessionManager) Reset(sessionID string) error {
	ctx := context.Background()

	// Delete messages
	if err := m.queries.DeleteSessionMessages(ctx, sessionID); err != nil {
		return err
	}

	// Reset session counters
	return m.queries.ResetSession(ctx, sessionID)
}

// ListSessions returns sessions, optionally filtered by userID
func (m *SessionManager) ListSessions(userID string) ([]AgentSession, error) {
	ctx := context.Background()

	var dbSessions []Session
	var err error

	if userID == "" {
		dbSessions, err = m.queries.ListSessionsByScope(ctx, sql.NullString{String: "agent", Valid: true})
	} else {
		dbSessions, err = m.queries.ListSessionsByScopeAndScopeID(ctx, ListSessionsByScopeAndScopeIDParams{
			Scope:   sql.NullString{String: "user", Valid: true},
			ScopeID: sql.NullString{String: userID, Valid: true},
		})
	}
	if err != nil {
		return nil, err
	}

	sessions := make([]AgentSession, 0, len(dbSessions))
	for _, dbSess := range dbSessions {
		sessions = append(sessions, AgentSession{
			ID:         dbSess.ID,
			SessionKey: dbSess.Name.String,
			CreatedAt:  time.Unix(dbSess.CreatedAt, 0),
			UpdatedAt:  time.Unix(dbSess.UpdatedAt, 0),
		})
	}
	return sessions, nil
}

// DeleteSession removes a session and all its messages
func (m *SessionManager) DeleteSession(sessionID string) error {
	ctx := context.Background()
	return m.queries.DeleteSession(ctx, sessionID)
}

// Close is a no-op since the database connection is shared
func (m *SessionManager) Close() error {
	return nil
}

// sanitizeAgentMessages removes orphaned tool_results that have no matching tool_calls
func sanitizeAgentMessages(messages []AgentMessage) []AgentMessage {
	if len(messages) == 0 {
		return messages
	}

	seenToolCallIDs := make(map[string]bool)

	result := make([]AgentMessage, 0, len(messages))
	for i, msg := range messages {
		if msg.Role == "assistant" && len(msg.ToolCalls) > 0 {
			var calls []AgentToolCall
			if err := json.Unmarshal(msg.ToolCalls, &calls); err == nil {
				for _, call := range calls {
					seenToolCallIDs[call.ID] = true
				}
			}
			result = append(result, msg)
			continue
		}

		if (msg.Role == "user" || msg.Role == "tool") && len(msg.ToolResults) > 0 {
			var results []AgentToolResult
			if err := json.Unmarshal(msg.ToolResults, &results); err == nil {
				validResults := make([]AgentToolResult, 0)
				for _, r := range results {
					if seenToolCallIDs[r.ToolCallID] {
						validResults = append(validResults, r)
					}
				}

				if len(validResults) == 0 {
					msg.ToolResults = nil
					if msg.Content == "" && i == 0 {
						continue
					}
				} else if len(validResults) < len(results) {
					if newResults, err := json.Marshal(validResults); err == nil {
						msg.ToolResults = newResults
					}
				}
			}
		}

		result = append(result, msg)
	}

	return result
}
