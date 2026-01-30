package session

import (
	"database/sql"
	"encoding/json"
	"fmt"
	"time"

	"github.com/google/uuid"
	_ "modernc.org/sqlite"
)

// Message represents a conversation message
type Message struct {
	ID          int64           `json:"id,omitempty"`
	SessionID   string          `json:"session_id"`
	Role        string          `json:"role"` // user, assistant, system, tool
	Content     string          `json:"content,omitempty"`
	ToolCalls   json.RawMessage `json:"tool_calls,omitempty"`
	ToolResults json.RawMessage `json:"tool_results,omitempty"`
	CreatedAt   time.Time       `json:"created_at"`
}

// ToolCall represents a tool invocation
type ToolCall struct {
	ID    string          `json:"id"`
	Name  string          `json:"name"`
	Input json.RawMessage `json:"input"`
}

// ToolResult represents the result of a tool execution
type ToolResult struct {
	ToolCallID string `json:"tool_call_id"`
	Content    string `json:"content"`
	IsError    bool   `json:"is_error,omitempty"`
}

// Session represents a conversation session
// Maps to server's sessions table schema
type Session struct {
	ID         string    `json:"id"`
	SessionKey string    `json:"session_key"` // Maps to 'name' column in DB
	CreatedAt  time.Time `json:"created_at"`
	UpdatedAt  time.Time `json:"updated_at"`
}

// Manager handles session persistence
// Uses the server's existing database schema (sessions + session_messages tables)
type Manager struct {
	db *sql.DB
}

// New creates a new session manager
// Expects the database to already have sessions and session_messages tables
// (created by server migrations)
func New(dbPath string) (*Manager, error) {
	db, err := sql.Open("sqlite", dbPath)
	if err != nil {
		return nil, fmt.Errorf("failed to open database: %w", err)
	}

	m := &Manager{db: db}

	// Verify tables exist (don't create them - server migrations handle that)
	if err := m.verifySchema(); err != nil {
		db.Close()
		return nil, fmt.Errorf("database schema verification failed: %w", err)
	}

	return m, nil
}

// verifySchema checks that the required tables exist
func (m *Manager) verifySchema() error {
	// Check sessions table exists
	var name string
	err := m.db.QueryRow("SELECT name FROM sqlite_master WHERE type='table' AND name='sessions'").Scan(&name)
	if err != nil {
		return fmt.Errorf("sessions table not found - run server migrations first: %w", err)
	}

	// Check session_messages table exists
	err = m.db.QueryRow("SELECT name FROM sqlite_master WHERE type='table' AND name='session_messages'").Scan(&name)
	if err != nil {
		return fmt.Errorf("session_messages table not found - run server migrations first: %w", err)
	}

	return nil
}

// Close closes the database connection
func (m *Manager) Close() error {
	return m.db.Close()
}

// GetOrCreate returns an existing session or creates a new one
// Uses server's sessions table schema: name column for session key, scope defaults to 'agent'
func (m *Manager) GetOrCreate(sessionKey string) (*Session, error) {
	// Try to get existing session by name
	session, err := m.getByKey(sessionKey)
	if err == nil {
		return session, nil
	}
	if err != sql.ErrNoRows {
		return nil, err
	}

	// Create new session using server's schema
	id := uuid.New().String()
	now := time.Now().Unix()
	_, err = m.db.Exec(
		`INSERT INTO sessions (id, name, scope, created_at, updated_at) VALUES (?, ?, 'agent', ?, ?)`,
		id, sessionKey, now, now,
	)
	if err != nil {
		return nil, fmt.Errorf("failed to create session: %w", err)
	}

	return &Session{
		ID:         id,
		SessionKey: sessionKey,
		CreatedAt:  time.Unix(now, 0),
		UpdatedAt:  time.Unix(now, 0),
	}, nil
}

// getByKey retrieves a session by its name (session key)
func (m *Manager) getByKey(sessionKey string) (*Session, error) {
	var s Session
	var createdAt, updatedAt int64
	err := m.db.QueryRow(
		"SELECT id, name, created_at, updated_at FROM sessions WHERE name = ?",
		sessionKey,
	).Scan(&s.ID, &s.SessionKey, &createdAt, &updatedAt)
	if err != nil {
		return nil, err
	}
	s.CreatedAt = time.Unix(createdAt, 0)
	s.UpdatedAt = time.Unix(updatedAt, 0)
	return &s, nil
}

// GetMessages retrieves messages for a session with an optional limit
// Uses server's session_messages table
func (m *Manager) GetMessages(sessionID string, limit int) ([]Message, error) {
	query := `
		SELECT id, session_id, role, content, tool_calls, tool_results, created_at
		FROM session_messages
		WHERE session_id = ?
		ORDER BY created_at ASC
	`
	if limit > 0 {
		// Get the last N messages
		query = `
			SELECT id, session_id, role, content, tool_calls, tool_results, created_at
			FROM (
				SELECT * FROM session_messages
				WHERE session_id = ?
				ORDER BY created_at DESC
				LIMIT ?
			) ORDER BY created_at ASC
		`
	}

	var rows *sql.Rows
	var err error
	if limit > 0 {
		rows, err = m.db.Query(query, sessionID, limit)
	} else {
		rows, err = m.db.Query(query, sessionID)
	}
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var messages []Message
	for rows.Next() {
		var msg Message
		var toolCalls, toolResults sql.NullString
		var createdAt int64
		err := rows.Scan(
			&msg.ID, &msg.SessionID, &msg.Role, &msg.Content,
			&toolCalls, &toolResults, &createdAt,
		)
		if err != nil {
			return nil, err
		}
		msg.CreatedAt = time.Unix(createdAt, 0)
		if toolCalls.Valid {
			msg.ToolCalls = json.RawMessage(toolCalls.String)
		}
		if toolResults.Valid {
			msg.ToolResults = json.RawMessage(toolResults.String)
		}
		messages = append(messages, msg)
	}

	return messages, rows.Err()
}

// AppendMessage adds a message to a session
// Uses server's session_messages table with integer timestamps
func (m *Manager) AppendMessage(sessionID string, msg Message) error {
	var toolCalls, toolResults sql.NullString
	if len(msg.ToolCalls) > 0 {
		toolCalls = sql.NullString{String: string(msg.ToolCalls), Valid: true}
	}
	if len(msg.ToolResults) > 0 {
		toolResults = sql.NullString{String: string(msg.ToolResults), Valid: true}
	}

	now := time.Now().Unix()
	_, err := m.db.Exec(
		"INSERT INTO session_messages (session_id, role, content, tool_calls, tool_results, created_at) VALUES (?, ?, ?, ?, ?, ?)",
		sessionID, msg.Role, msg.Content, toolCalls, toolResults, now,
	)
	if err != nil {
		return fmt.Errorf("failed to append message: %w", err)
	}

	// Update session timestamp and message count
	_, err = m.db.Exec(
		"UPDATE sessions SET updated_at = ?, message_count = message_count + 1 WHERE id = ?",
		now, sessionID,
	)
	return err
}

// Compact summarizes old messages to reduce context size
// This should be called when context window is getting full
// Uses server's session_messages table and updates session summary
func (m *Manager) Compact(sessionID string, summaryContent string) error {
	tx, err := m.db.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback()

	// Get message count
	var count int
	err = tx.QueryRow("SELECT COUNT(*) FROM session_messages WHERE session_id = ?", sessionID).Scan(&count)
	if err != nil {
		return err
	}

	// Keep the last 10 messages, summarize the rest
	keepCount := 10
	if count <= keepCount {
		return nil // Nothing to compact
	}

	// Mark old messages as compacted (instead of deleting, per server schema)
	_, err = tx.Exec(`
		UPDATE session_messages
		SET is_compacted = 1
		WHERE session_id = ? AND id NOT IN (
			SELECT id FROM session_messages
			WHERE session_id = ?
			ORDER BY created_at DESC
			LIMIT ?
		)
	`, sessionID, sessionID, keepCount)
	if err != nil {
		return err
	}

	// Update session with summary and compaction timestamp
	now := time.Now().Unix()
	_, err = tx.Exec(
		"UPDATE sessions SET summary = ?, last_compacted_at = ?, updated_at = ? WHERE id = ?",
		summaryContent, now, now, sessionID,
	)
	if err != nil {
		return err
	}

	return tx.Commit()
}

// Reset clears all messages from a session
func (m *Manager) Reset(sessionID string) error {
	tx, err := m.db.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback()

	// Delete messages
	_, err = tx.Exec("DELETE FROM session_messages WHERE session_id = ?", sessionID)
	if err != nil {
		return err
	}

	// Reset session counters
	now := time.Now().Unix()
	_, err = tx.Exec(
		"UPDATE sessions SET message_count = 0, token_count = 0, summary = NULL, last_compacted_at = NULL, updated_at = ? WHERE id = ?",
		now, sessionID,
	)
	if err != nil {
		return err
	}

	return tx.Commit()
}

// ListSessions returns all sessions (filtered to agent scope)
func (m *Manager) ListSessions() ([]Session, error) {
	rows, err := m.db.Query(
		"SELECT id, name, created_at, updated_at FROM sessions WHERE scope = 'agent' ORDER BY updated_at DESC",
	)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var sessions []Session
	for rows.Next() {
		var s Session
		var createdAt, updatedAt int64
		var name sql.NullString
		if err := rows.Scan(&s.ID, &name, &createdAt, &updatedAt); err != nil {
			return nil, err
		}
		s.SessionKey = name.String
		s.CreatedAt = time.Unix(createdAt, 0)
		s.UpdatedAt = time.Unix(updatedAt, 0)
		sessions = append(sessions, s)
	}
	return sessions, rows.Err()
}

// DeleteSession removes a session and all its messages
// Uses CASCADE delete from session_messages via foreign key
func (m *Manager) DeleteSession(sessionID string) error {
	// session_messages has ON DELETE CASCADE, so just delete the session
	_, err := m.db.Exec("DELETE FROM sessions WHERE id = ?", sessionID)
	return err
}
