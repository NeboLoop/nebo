package db

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"sync"
	"time"

	"github.com/google/uuid"
)

// AgentMessage represents a conversation message for the agent.
// Now backed by the chat_messages table (unified storage).
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

// SessionManager handles session persistence using sqlc-generated queries.
// Messages are stored in chat_messages (unified storage).
// Sessions table holds metadata only (active_task, model_override, scope).
type SessionManager struct {
	queries *Queries
	rawDB   *sql.DB
	// sessionKeys caches sessionID → sessionKey mappings.
	// The runner calls AppendMessage(sessionID, ...) but chat_messages
	// uses sessionKey as chat_id. This cache avoids a DB lookup per write.
	sessionKeys sync.Map // map[string]string
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

// PurgeEmptyMessages removes chat messages that have no content, no tool calls,
// and no tool results. These ghost records accumulate from failed runs and confuse
// onboarding/introduction logic.
func (m *SessionManager) PurgeEmptyMessages() (int64, error) {
	result, err := m.rawDB.Exec(`
		DELETE FROM chat_messages
		WHERE (content IS NULL OR content = '')
		  AND (tool_calls IS NULL OR tool_calls = '')
		  AND (tool_results IS NULL OR tool_results = '')
	`)
	if err != nil {
		return 0, err
	}
	return result.RowsAffected()
}

// resolveSessionKey looks up the sessionKey for a given sessionID.
// Uses the in-memory cache first, then falls back to a DB query.
func (m *SessionManager) resolveSessionKey(sessionID string) (string, error) {
	if key, ok := m.sessionKeys.Load(sessionID); ok {
		return key.(string), nil
	}
	ctx := context.Background()
	sess, err := m.queries.GetSession(ctx, sessionID)
	if err != nil {
		return "", fmt.Errorf("failed to resolve session key for %s: %w", sessionID, err)
	}
	key := sess.Name.String
	m.sessionKeys.Store(sessionID, key)
	return key, nil
}

// ensureChatExists creates a chats row for a sessionKey if one doesn't exist.
// This ensures chat_messages foreign key constraints are satisfied.
func (m *SessionManager) ensureChatExists(ctx context.Context, sessionKey string) {
	// INSERT OR IGNORE — no error if it already exists
	_, _ = m.rawDB.ExecContext(ctx,
		`INSERT OR IGNORE INTO chats (id, title, created_at, updated_at) VALUES (?, ?, unixepoch(), unixepoch())`,
		sessionKey, "Session: "+sessionKey,
	)
}

// GetOrCreate returns an existing session or creates a new one.
// If userID is provided, session is scoped to that user; otherwise uses agent scope.
// Also ensures a matching chats row exists for unified message storage.
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
		m.sessionKeys.Store(dbSession.ID, sessionKey)
		// Ensure chats row exists for message storage
		m.ensureChatExists(ctx, sessionKey)
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

	m.sessionKeys.Store(id, sessionKey)
	// Ensure chats row exists for message storage
	m.ensureChatExists(ctx, sessionKey)

	return &AgentSession{
		ID:         dbSession.ID,
		SessionKey: dbSession.Name.String,
		CreatedAt:  time.Unix(dbSession.CreatedAt, 0),
		UpdatedAt:  time.Unix(dbSession.UpdatedAt, 0),
	}, nil
}

// GetSummary retrieves the rolling summary for a session (if any).
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

// GetMessages retrieves messages for a session with an optional limit.
// Reads from chat_messages using sessionKey as chat_id.
func (m *SessionManager) GetMessages(sessionID string, limit int) ([]AgentMessage, error) {
	ctx := context.Background()

	sessionKey, err := m.resolveSessionKey(sessionID)
	if err != nil {
		return nil, err
	}

	var dbMessages []ChatMessage
	if limit > 0 {
		dbMessages, err = m.queries.GetRecentChatMessagesWithTools(ctx, GetRecentChatMessagesWithToolsParams{
			ChatID: sessionKey,
			Limit:  int64(limit),
		})
	} else {
		dbMessages, err = m.queries.GetChatMessages(ctx, sessionKey)
	}
	if err != nil {
		return nil, err
	}

	messages := make([]AgentMessage, 0, len(dbMessages))
	for _, dbMsg := range dbMessages {
		msg := AgentMessage{
			ID:        dbMsg.CreatedAt, // Use created_at as ordering key (chat_messages.id is TEXT UUID)
			SessionID: sessionID,
			Role:      dbMsg.Role,
			Content:   dbMsg.Content,
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

// AppendMessage adds a message to a session.
// Writes to chat_messages using sessionKey as chat_id.
func (m *SessionManager) AppendMessage(sessionID string, msg AgentMessage) error {
	// Guard against saving truly empty messages (no content, no tool data).
	// These create ghost records that confuse introduction/onboarding checks.
	if msg.Content == "" && len(msg.ToolCalls) == 0 && len(msg.ToolResults) == 0 {
		return nil // silently skip
	}

	ctx := context.Background()

	sessionKey, err := m.resolveSessionKey(sessionID)
	if err != nil {
		return err
	}

	var toolCalls, toolResults sql.NullString
	if len(msg.ToolCalls) > 0 {
		toolCalls = sql.NullString{String: string(msg.ToolCalls), Valid: true}
	}
	if len(msg.ToolResults) > 0 {
		toolResults = sql.NullString{String: string(msg.ToolResults), Valid: true}
	}

	msgID := uuid.New().String()
	_, err = m.queries.CreateChatMessageForRunner(ctx, CreateChatMessageForRunnerParams{
		ID:          msgID,
		ChatID:      sessionKey,
		Role:        msg.Role,
		Content:     msg.Content,
		ToolCalls:   toolCalls,
		ToolResults: toolResults,
	})
	if err != nil {
		return fmt.Errorf("failed to append message: %w", err)
	}

	// Update session message count
	return m.queries.IncrementSessionMessageCount(ctx, sessionID)
}

// GetActiveTask returns the pinned active task for a session
func (m *SessionManager) GetActiveTask(sessionID string) (string, error) {
	ctx := context.Background()
	task, err := m.queries.GetSessionActiveTask(ctx, sessionID)
	if err != nil {
		return "", err
	}
	return task, nil
}

// SetActiveTask pins a task description to the session
func (m *SessionManager) SetActiveTask(sessionID, task string) error {
	ctx := context.Background()
	return m.queries.SetSessionActiveTask(ctx, SetSessionActiveTaskParams{
		ActiveTask: sql.NullString{String: task, Valid: task != ""},
		ID:         sessionID,
	})
}

// ClearActiveTask removes the pinned task from a session
func (m *SessionManager) ClearActiveTask(sessionID string) error {
	ctx := context.Background()
	return m.queries.ClearSessionActiveTask(ctx, sessionID)
}

// Reset clears all messages from a session.
// Deletes from chat_messages using sessionKey as chat_id.
func (m *SessionManager) Reset(sessionID string) error {
	ctx := context.Background()

	sessionKey, err := m.resolveSessionKey(sessionID)
	if err != nil {
		// Fallback: try to delete anyway with session ID
		// (shouldn't happen, but be defensive)
		return m.queries.ResetSession(ctx, sessionID)
	}

	// Delete messages from chat_messages
	if err := m.queries.DeleteChatMessagesByChatId(ctx, sessionKey); err != nil {
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
	// Also clean up chat_messages
	if sessionKey, err := m.resolveSessionKey(sessionID); err == nil {
		_ = m.queries.DeleteChatMessagesByChatId(ctx, sessionKey)
	}
	return m.queries.DeleteSession(ctx, sessionID)
}

// Close is a no-op since the database connection is shared
func (m *SessionManager) Close() error {
	return nil
}

// GetLastSummarizedCount returns how many messages have been incorporated into the rolling summary.
// Returns (0, nil) for sessions that predate the migration.
func (m *SessionManager) GetLastSummarizedCount(sessionID string) (int, error) {
	ctx := context.Background()
	var count sql.NullInt64
	err := m.rawDB.QueryRowContext(ctx,
		`SELECT last_summarized_count FROM sessions WHERE id = ?`, sessionID,
	).Scan(&count)
	if err != nil {
		return 0, nil // Graceful fallback for pre-migration sessions
	}
	if count.Valid {
		return int(count.Int64), nil
	}
	return 0, nil
}

// SetLastSummarizedCount records how many messages have been incorporated into the rolling summary.
func (m *SessionManager) SetLastSummarizedCount(sessionID string, count int) error {
	ctx := context.Background()
	_, err := m.rawDB.ExecContext(ctx,
		`UPDATE sessions SET last_summarized_count = ? WHERE id = ?`,
		count, sessionID,
	)
	return err
}

// UpdateSummary updates the session's summary without compacting messages.
// Used by the sliding window to persist rolling summaries independently.
func (m *SessionManager) UpdateSummary(sessionID, summary string) error {
	ctx := context.Background()
	_, err := m.rawDB.ExecContext(ctx,
		`UPDATE sessions SET summary = ? WHERE id = ?`,
		summary, sessionID,
	)
	return err
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
